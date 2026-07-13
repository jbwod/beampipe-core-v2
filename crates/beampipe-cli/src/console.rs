use anyhow::{bail, Context, Result};
use beampipe_config::Settings;
use beampipe_db::{
    models::{
        ExecutionRow, ExecutionStatePatch, OperatorOverviewCounts, ProvenanceEventRow,
        SourceRegistryRow, WorkerInstanceRow, WorkerPoolSummary,
    },
    repo,
};
use beampipe_domain::{DaliugeState, ExecutionStatus, LedgerPatch, SchedulerState};
use beampipe_orchestration::{
    cancel_scheduler_session, CancelParams, DaliugeManager, DaliugeTranslator, HttpDimClient,
    HttpTranslatorClient, SchedulerAdapter, SshSlurmClient,
};
use beampipe_profiles::DeploymentConfig;
use chrono::{DateTime, Utc};
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyModifiers,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table, Tabs, Wrap},
    Frame, Terminal,
};
use sqlx::PgPool;
use std::{io, time::Duration};
use uuid::Uuid;

const VIEW_COUNT: usize = 7;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum View {
    Overview,
    Sources,
    Executions,
    Workers,
    Scheduler,
    Daliuge,
    Logs,
}

impl View {
    const ALL: [Self; VIEW_COUNT] = [
        Self::Overview,
        Self::Sources,
        Self::Executions,
        Self::Workers,
        Self::Scheduler,
        Self::Daliuge,
        Self::Logs,
    ];

    fn index(self) -> usize {
        Self::ALL.iter().position(|view| *view == self).unwrap_or(0)
    }

    fn title(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Sources => "Sources",
            Self::Executions => "Executions",
            Self::Workers => "Workers",
            Self::Scheduler => "Scheduler",
            Self::Daliuge => "DALiuGE",
            Self::Logs => "Logs",
        }
    }
}

#[derive(Debug, Clone)]
enum PendingAction {
    WorkerDrain { id: Uuid, draining: bool },
    ExecutionCancel { id: Uuid },
    ExecutionRetry { id: Uuid },
}

#[derive(Debug)]
struct ConsoleState {
    view: View,
    selected: [usize; VIEW_COUNT],
    filter: String,
    editing_filter: bool,
    paused: bool,
    show_help: bool,
    show_detail: bool,
    pending_action: Option<PendingAction>,
    notice: String,
    refresh_requested: bool,
}

impl Default for ConsoleState {
    fn default() -> Self {
        Self {
            view: View::Overview,
            selected: [0; VIEW_COUNT],
            filter: String::new(),
            editing_filter: false,
            paused: false,
            show_help: false,
            show_detail: false,
            pending_action: None,
            notice: "connecting".into(),
            refresh_requested: true,
        }
    }
}

impl ConsoleState {
    fn next_view(&mut self, reverse: bool) {
        let current = self.view.index();
        let next = if reverse {
            (current + VIEW_COUNT - 1) % VIEW_COUNT
        } else {
            (current + 1) % VIEW_COUNT
        };
        self.view = View::ALL[next];
    }

    fn move_selection(&mut self, delta: isize) {
        let selected = &mut self.selected[self.view.index()];
        *selected = selected.saturating_add_signed(delta);
    }
}

#[derive(Debug, Default)]
struct IntegrationData {
    translator: String,
    manager: String,
    scheduler: String,
    sessions: Vec<(String, String, String)>,
}

#[derive(Debug, Default)]
struct ConsoleData {
    overview: Option<OperatorOverviewCounts>,
    sources: Vec<SourceRegistryRow>,
    executions: Vec<ExecutionRow>,
    workers: Vec<WorkerInstanceRow>,
    pools: Vec<WorkerPoolSummary>,
    scheduler_jobs: Vec<ExecutionRow>,
    events: Vec<ProvenanceEventRow>,
    integrations: IntegrationData,
    refreshed_at: Option<DateTime<Utc>>,
}

pub async fn run(refresh_ms: u64) -> Result<()> {
    let settings = Settings::load()?.settings;
    let pool = beampipe_db::connect(&settings.database_url).await?;

    enable_raw_mode().context("enable terminal raw mode")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_loop(
        &mut terminal,
        &pool,
        &settings,
        Duration::from_millis(refresh_ms.clamp(250, 60_000)),
    )
    .await;

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;
    result
}

async fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    pool: &PgPool,
    settings: &Settings,
    refresh_interval: Duration,
) -> Result<()> {
    let mut state = ConsoleState::default();
    let mut data = ConsoleData::default();
    let mut last_refresh = tokio::time::Instant::now() - refresh_interval;
    let mut last_integration_refresh = tokio::time::Instant::now() - Duration::from_secs(30);

    loop {
        if !state.paused && (state.refresh_requested || last_refresh.elapsed() >= refresh_interval)
        {
            match refresh_database(pool, settings, &mut data).await {
                Ok(()) => state.notice = "live".into(),
                Err(error) => state.notice = format!("refresh failed: {error}"),
            }
            last_refresh = tokio::time::Instant::now();
            state.refresh_requested = false;
        }
        if !state.paused
            && (last_integration_refresh.elapsed() >= Duration::from_secs(30)
                || (state.view == View::Daliuge && data.integrations.manager.is_empty()))
        {
            refresh_integrations(pool, settings, &mut data.integrations).await;
            last_integration_refresh = tokio::time::Instant::now();
        }

        terminal.draw(|frame| render(frame, &state, &data))?;
        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if handle_key(pool, &mut state, &data, key).await? {
                    return Ok(());
                }
            }
        }
    }
}

async fn refresh_database(
    pool: &PgPool,
    settings: &Settings,
    data: &mut ConsoleData,
) -> Result<()> {
    let stale_after = (settings.worker_heartbeat_interval_seconds as i64 * 3).max(30);
    let (overview, sources, executions, workers, pools, scheduler_jobs, events) = tokio::try_join!(
        repo::operator_overview_counts(pool, stale_after),
        repo::list_sources(pool, None, 500, 0),
        repo::list_executions(pool, None, None, 1, 500),
        repo::list_worker_instances(pool, false),
        repo::list_worker_pools(pool),
        repo::list_scheduler_executions(pool, "slurm", 500, 0),
        repo::list_recent_provenance_events(pool, 500, 0),
    )?;
    data.overview = Some(overview);
    data.sources = sources;
    data.executions = executions.items;
    data.workers = workers;
    data.pools = pools;
    data.scheduler_jobs = scheduler_jobs;
    data.events = events;
    data.refreshed_at = Some(Utc::now());
    Ok(())
}

async fn refresh_integrations(pool: &PgPool, settings: &Settings, data: &mut IntegrationData) {
    data.sessions.clear();
    data.translator = match settings.tm_url.as_deref() {
        Some(url) => match tokio::time::timeout(
            Duration::from_secs(4),
            HttpTranslatorClient::new(url).inspect(None, None),
        )
        .await
        {
            Ok(Ok(info)) => format!(
                "ready version={} updated_api={}",
                info.version.as_deref().unwrap_or("unreported"),
                info.capabilities.updated_translation_api
            ),
            Ok(Err(error)) => format!("error: {}", bounded(&error.to_string(), 100)),
            Err(_) => "timeout".into(),
        },
        None => "not configured".into(),
    };
    data.manager = match settings.dim_url.as_deref() {
        Some(url) => {
            match tokio::time::timeout(Duration::from_secs(4), HttpDimClient::new(url).inspect())
                .await
            {
                Ok(Ok(info)) => {
                    data.sessions = info
                        .sessions
                        .iter()
                        .map(|session| {
                            (
                                session.session_id.clone(),
                                format!("{:?}", session.state()).to_ascii_lowercase(),
                                session.size.to_string(),
                            )
                        })
                        .collect();
                    format!(
                        "ready version={} nodes={} sessions={}",
                        info.version.as_deref().unwrap_or("unreported"),
                        info.nodes.len(),
                        info.sessions.len()
                    )
                }
                Ok(Err(error)) => format!("error: {}", bounded(&error.to_string(), 100)),
                Err(_) => "timeout".into(),
            }
        }
        None => "not configured".into(),
    };

    data.scheduler = "no SLURM profile".into();
    let profiles = repo::list_deployment_profiles(pool, None, 500, 0)
        .await
        .unwrap_or_default();
    if let Some((name, slurm)) = profiles.into_iter().find_map(|row| {
        serde_json::from_value::<DeploymentConfig>(row.deployment)
            .ok()
            .and_then(|deployment| match deployment {
                DeploymentConfig::SlurmRemote(slurm) => Some((row.name, slurm)),
                DeploymentConfig::RestRemote(_) => None,
            })
    }) {
        if !settings.use_real_backends {
            data.scheduler = format!("profile={name} live probe disabled");
        } else {
            let client = SshSlurmClient {
                login_node: slurm.login_node.clone(),
                remote_user: slurm.remote_user.clone(),
                session_dir: slurm.log_dir.clone(),
                account: Some(slurm.account.clone()),
                ssh_port: slurm.ssh_port,
                dlg_root: slurm.dlg_root.clone(),
                deployment: Some(slurm),
            };
            data.scheduler = match tokio::time::timeout(
                Duration::from_secs(8),
                client.test_connectivity(),
            )
            .await
            {
                Ok(Ok(info)) => format!(
                    "ready profile={name} version={}",
                    info.scheduler_version.as_deref().unwrap_or("unreported")
                ),
                Ok(Err(error)) => format!("error: {}", bounded(&error.to_string(), 100)),
                Err(_) => "timeout".into(),
            };
        }
    }
}

async fn handle_key(
    pool: &PgPool,
    state: &mut ConsoleState,
    data: &ConsoleData,
    key: KeyEvent,
) -> Result<bool> {
    if state.pending_action.is_some() {
        match key.code {
            KeyCode::Char('y') | KeyCode::Enter => {
                let action = state.pending_action.take().expect("checked above");
                state.notice = match apply_action(pool, data, action).await {
                    Ok(message) => message,
                    Err(error) => format!("action failed: {error}"),
                };
                state.refresh_requested = true;
            }
            KeyCode::Char('n') | KeyCode::Esc => {
                state.pending_action = None;
                state.notice = "action cancelled".into();
            }
            _ => {}
        }
        return Ok(false);
    }
    if state.editing_filter {
        match key.code {
            KeyCode::Esc => state.editing_filter = false,
            KeyCode::Enter => {
                state.editing_filter = false;
                state.selected[state.view.index()] = 0;
            }
            KeyCode::Backspace => {
                state.filter.pop();
            }
            KeyCode::Char(character) => state.filter.push(character),
            _ => {}
        }
        return Ok(false);
    }
    match key.code {
        KeyCode::Char('q') => return Ok(true),
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => return Ok(true),
        KeyCode::Tab => state.next_view(false),
        KeyCode::BackTab => state.next_view(true),
        KeyCode::Right => state.next_view(false),
        KeyCode::Left => state.next_view(true),
        KeyCode::Down | KeyCode::Char('j') => state.move_selection(1),
        KeyCode::Up | KeyCode::Char('k') => state.move_selection(-1),
        KeyCode::Char('/') => state.editing_filter = true,
        KeyCode::Char('p') => {
            state.paused = !state.paused;
            state.notice = if state.paused { "paused" } else { "live" }.into();
        }
        KeyCode::Char('r') => state.refresh_requested = true,
        KeyCode::Char('?') => state.show_help = !state.show_help,
        KeyCode::Enter => state.show_detail = true,
        KeyCode::Esc => {
            state.show_help = false;
            state.show_detail = false;
            state.filter.clear();
        }
        KeyCode::Char('d') if state.view == View::Workers => {
            if let Some(worker) =
                filtered_workers(data, &state.filter).get(state.selected[View::Workers.index()])
            {
                let draining = worker.status != "draining";
                state.pending_action = Some(PendingAction::WorkerDrain {
                    id: worker.uuid,
                    draining,
                });
            }
        }
        KeyCode::Char('c') if state.view == View::Executions => {
            if let Some(execution) = filtered_executions(data, &state.filter)
                .get(state.selected[View::Executions.index()])
                .filter(|execution| {
                    matches!(
                        execution.status_enum(),
                        Some(ExecutionStatus::Running | ExecutionStatus::AwaitingScheduler)
                    )
                })
            {
                state.pending_action = Some(PendingAction::ExecutionCancel { id: execution.uuid });
            }
        }
        KeyCode::Char('R') if state.view == View::Executions => {
            if let Some(execution) = filtered_executions(data, &state.filter)
                .get(state.selected[View::Executions.index()])
                .filter(|execution| execution.status_enum() == Some(ExecutionStatus::Failed))
            {
                state.pending_action = Some(PendingAction::ExecutionRetry { id: execution.uuid });
            }
        }
        _ => {}
    }
    Ok(false)
}

async fn apply_action(pool: &PgPool, data: &ConsoleData, action: PendingAction) -> Result<String> {
    match action {
        PendingAction::WorkerDrain { id, draining } => {
            let worker = repo::set_worker_draining(pool, id, draining)
                .await?
                .ok_or_else(|| anyhow::anyhow!("worker is missing or stopped"))?;
            repo::insert_provenance_event(
                pool,
                if draining {
                    "worker.drained"
                } else {
                    "worker.resumed"
                },
                "system",
                None,
                None,
                Some("operator:console"),
                Some(&id.to_string()),
                &serde_json::json!({"worker_id": id, "draining": draining}),
            )
            .await?;
            Ok(format!(
                "worker {} is {}",
                short_id(worker.uuid),
                worker.status
            ))
        }
        PendingAction::ExecutionCancel { id } => {
            let execution = data
                .executions
                .iter()
                .find(|execution| execution.uuid == id)
                .cloned()
                .or(repo::get_execution(pool, id).await?)
                .ok_or_else(|| anyhow::anyhow!("execution not found"))?;
            let deployment = repo::resolve_execution_deployment(pool, &execution)
                .await?
                .ok_or_else(|| anyhow::anyhow!("execution has no pinned deployment profile"))?;
            let deployment_kind = serde_json::from_value::<DeploymentConfig>(deployment.clone())?;
            let result = cancel_scheduler_session(CancelParams {
                scheduler_job_id: execution.scheduler_job_id.clone(),
                daliuge_session_id: execution.daliuge_session_id.clone(),
                deployment,
            })
            .await?;
            if !result.cancelled {
                bail!(
                    "external cancellation was not confirmed: {}",
                    result.reason.unwrap_or_else(|| "unknown reason".into())
                );
            }
            let state_patch = match deployment_kind {
                DeploymentConfig::RestRemote(_) => ExecutionStatePatch {
                    daliuge_state: Some(DaliugeState::Cancelled),
                    ..Default::default()
                },
                DeploymentConfig::SlurmRemote(_) => ExecutionStatePatch {
                    scheduler_state: Some(SchedulerState::Cancelled),
                    ..Default::default()
                },
            };
            repo::apply_execution_state_patch(pool, id, state_patch).await?;
            repo::apply_execution_patch(
                pool,
                id,
                LedgerPatch {
                    status: Some(ExecutionStatus::Cancelled),
                    ..Default::default()
                },
            )
            .await?;
            repo::insert_provenance_event(
                pool,
                "execution.cancelled",
                &execution.project_module,
                None,
                Some(id),
                Some("operator:console"),
                Some(&id.to_string()),
                &serde_json::json!({
                    "scheduler_job_id": execution.scheduler_job_id,
                    "daliuge_session_id": execution.daliuge_session_id,
                }),
            )
            .await?;
            Ok(format!("execution {} cancelled", short_id(id)))
        }
        PendingAction::ExecutionRetry { id } => {
            let result = repo::retry_execution(
                pool,
                id,
                "operator:console",
                "operator confirmed retry after inspecting the failed execution",
                Some(&id.to_string()),
            )
            .await?;
            Ok(format!(
                "execution {} retry {} queued from {}",
                short_id(id),
                result.execution.retry_count,
                result.plan.stage.as_str()
            ))
        }
    }
}

fn render(frame: &mut Frame<'_>, state: &ConsoleState, data: &ConsoleData) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(3),
            Constraint::Length(2),
        ])
        .split(frame.area());
    let tabs = Tabs::new(
        View::ALL
            .iter()
            .map(|view| Line::from(view.title()))
            .collect::<Vec<_>>(),
    )
    .select(state.view.index())
    .block(
        Block::default()
            .title(" Beampipe operator console ")
            .borders(Borders::ALL),
    )
    .highlight_style(
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    );
    frame.render_widget(tabs, chunks[0]);

    if frame.area().width < 70 {
        render_narrow(frame, chunks[1], state, data);
    } else {
        match state.view {
            View::Overview => render_overview(frame, chunks[1], data),
            View::Sources => render_sources(frame, chunks[1], state, data),
            View::Executions => render_executions(frame, chunks[1], state, data),
            View::Workers => render_workers(frame, chunks[1], state, data),
            View::Scheduler => render_scheduler(frame, chunks[1], state, data),
            View::Daliuge => render_daliuge(frame, chunks[1], state, data),
            View::Logs => render_logs(frame, chunks[1], state, data),
        }
    }

    let refreshed = data
        .refreshed_at
        .map(|at| at.format("%H:%M:%S UTC").to_string())
        .unwrap_or_else(|| "never".into());
    let mode = if state.paused { "PAUSED" } else { "LIVE" };
    let footer = if state.editing_filter {
        format!("filter> {}", state.filter)
    } else {
        format!(
            "{mode} | {} | refreshed {refreshed} | Tab views  / filter  r refresh  ? help  q quit",
            state.notice
        )
    };
    frame.render_widget(
        Paragraph::new(footer).style(Style::default().fg(Color::Gray)),
        chunks[2],
    );

    if state.show_help {
        render_popup(
            frame,
            "Help",
            "Tab/Shift-Tab or arrows: views\nj/k or arrows: selection\n/: filter, Esc: clear\np: pause live refresh, r: refresh\nWorkers d: drain/resume\nExecutions c: cancel active run, R: retry failed stage\n?: close help, q: quit",
        );
    }
    if state.show_detail {
        render_popup(frame, state.view.title(), &detail_text(state, data));
    }
    if let Some(action) = &state.pending_action {
        let message = match action {
            PendingAction::WorkerDrain { draining, .. } => {
                if *draining {
                    "Drain selected Beampipe worker?"
                } else {
                    "Resume selected Beampipe worker?"
                }
            }
            PendingAction::ExecutionCancel { .. } => {
                "Cancel the selected external scheduler/DALiuGE execution?"
            }
            PendingAction::ExecutionRetry { .. } => {
                "Retry the last safe failed stage? Uncertain external work will be refused."
            }
        };
        render_popup(
            frame,
            "Confirm",
            &format!("{message}\n\nPress y/Enter to confirm, n/Esc to abort."),
        );
    }
}

fn render_overview(frame: &mut Frame<'_>, area: Rect, data: &ConsoleData) {
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);
    let counts = data.overview.as_ref().map_or_else(
        || "Waiting for PostgreSQL".into(),
        |value| {
            format!(
                "Registered sources   {:>8}\nPending admissions   {:>8}\nRunning executions   {:>8}\nFailed executions    {:>8}\nQueue depth          {:>8}\nActive workers       {:>8}\nStale workers        {:>8}\nAlerts (24h)         {:>8}",
                value.registered_sources,
                value.pending_admissions,
                value.running_executions,
                value.failed_executions,
                value.queue_depth,
                value.active_workers,
                value.stale_workers,
                value.recent_alerts,
            )
        },
    );
    frame.render_widget(panel("Control plane", counts), columns[0]);
    let integrations = format!(
        "CASDA discovery  configured sources={}\nTranslator       {}\nData Island Mgr  {}\nSLURM            {}\n\nWorker pools\n{}",
        data.sources.len(),
        data.integrations.translator,
        data.integrations.manager,
        data.integrations.scheduler,
        data.pools
            .iter()
            .map(|pool| format!(
                "{}: workers={} leases={}/{}",
                pool.pool, pool.active_workers, pool.active_leases, pool.concurrency_limit
            ))
            .collect::<Vec<_>>()
            .join("\n")
    );
    frame.render_widget(panel("External systems", integrations), columns[1]);
}

fn render_sources(frame: &mut Frame<'_>, area: Rect, state: &ConsoleState, data: &ConsoleData) {
    let rows = filtered_sources(data, &state.filter);
    let table_rows = rows.iter().map(|source| {
        Row::new(vec![
            Cell::from(source.source_identifier.clone()),
            Cell::from(source.project_module.clone()),
            Cell::from(source_state(source)),
            Cell::from(short_hash(source.discovery_signature.as_deref())),
            Cell::from(format_time(source.last_checked_at)),
        ])
    });
    render_table(
        frame,
        area,
        "Sources",
        [
            "Identifier",
            "Project",
            "Readiness",
            "Signature",
            "Last discovery",
        ],
        table_rows,
        &[
            Constraint::Percentage(24),
            Constraint::Percentage(18),
            Constraint::Percentage(22),
            Constraint::Length(12),
            Constraint::Percentage(24),
        ],
        state.selected[View::Sources.index()],
    );
}

fn render_executions(frame: &mut Frame<'_>, area: Rect, state: &ConsoleState, data: &ConsoleData) {
    let rows = filtered_executions(data, &state.filter);
    let table_rows = rows.iter().map(|execution| {
        Row::new(vec![
            Cell::from(short_id(execution.uuid)),
            Cell::from(execution.project_module.clone()),
            Cell::from(execution.status.clone()),
            Cell::from(
                execution
                    .control_phase
                    .clone()
                    .unwrap_or_else(|| "-".into()),
            ),
            Cell::from(
                execution
                    .scheduler_state
                    .clone()
                    .unwrap_or_else(|| "-".into()),
            ),
            Cell::from(
                execution
                    .daliuge_state
                    .clone()
                    .unwrap_or_else(|| "-".into()),
            ),
            Cell::from(execution.output_state.clone().unwrap_or_else(|| "-".into())),
        ])
        .style(state_style(&execution.status))
    });
    render_table(
        frame,
        area,
        "Executions (c cancel, R retry failed stage)",
        [
            "Execution",
            "Project",
            "Status",
            "Phase",
            "Scheduler",
            "DALiuGE",
            "Outputs",
        ],
        table_rows,
        &[
            Constraint::Length(10),
            Constraint::Percentage(16),
            Constraint::Length(18),
            Constraint::Percentage(18),
            Constraint::Length(12),
            Constraint::Length(12),
            Constraint::Length(12),
        ],
        state.selected[View::Executions.index()],
    );
}

fn render_workers(frame: &mut Frame<'_>, area: Rect, state: &ConsoleState, data: &ConsoleData) {
    let rows = filtered_workers(data, &state.filter);
    let table_rows = rows.iter().map(|worker| {
        let age = Utc::now()
            .signed_duration_since(worker.last_heartbeat_at)
            .num_seconds();
        Row::new(vec![
            Cell::from(short_id(worker.uuid)),
            Cell::from(worker.instance_name.clone()),
            Cell::from(worker.host_name.clone()),
            Cell::from(worker.pool.clone()),
            Cell::from(worker.status.clone()),
            Cell::from(format!("{}s", age.max(0))),
            Cell::from(worker.capabilities.join(",")),
        ])
        .style(state_style(&worker.status))
    });
    render_table(
        frame,
        area,
        "Beampipe control-plane workers (d drains/resumes)",
        [
            "Worker",
            "Instance",
            "Host",
            "Pool",
            "State",
            "Heartbeat",
            "Capabilities",
        ],
        table_rows,
        &[
            Constraint::Length(10),
            Constraint::Percentage(20),
            Constraint::Percentage(15),
            Constraint::Length(12),
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Percentage(30),
        ],
        state.selected[View::Workers.index()],
    );
}

fn render_scheduler(frame: &mut Frame<'_>, area: Rect, state: &ConsoleState, data: &ConsoleData) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(3)])
        .split(area);
    frame.render_widget(
        panel("SLURM connectivity", data.integrations.scheduler.clone()),
        chunks[0],
    );
    let needle = state.filter.to_ascii_lowercase();
    let rows: Vec<_> = data
        .scheduler_jobs
        .iter()
        .filter(|execution| execution_matches(execution, &needle))
        .collect();
    let table_rows = rows.iter().map(|execution| {
        Row::new(vec![
            Cell::from(short_id(execution.uuid)),
            Cell::from(
                execution
                    .scheduler_job_id
                    .clone()
                    .unwrap_or_else(|| "-".into()),
            ),
            Cell::from(
                execution
                    .scheduler_state
                    .clone()
                    .unwrap_or_else(|| "unknown".into()),
            ),
            Cell::from(
                execution
                    .scheduler_reason
                    .clone()
                    .unwrap_or_else(|| "-".into()),
            ),
            Cell::from(format_time(execution.last_reconciled_at)),
        ])
    });
    render_table(
        frame,
        chunks[1],
        "Scheduler jobs",
        ["Execution", "Job ID", "State", "Reason", "Reconciled"],
        table_rows,
        &[
            Constraint::Length(10),
            Constraint::Length(18),
            Constraint::Length(14),
            Constraint::Percentage(40),
            Constraint::Length(20),
        ],
        state.selected[View::Scheduler.index()],
    );
}

fn render_daliuge(frame: &mut Frame<'_>, area: Rect, state: &ConsoleState, data: &ConsoleData) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(4), Constraint::Min(3)])
        .split(area);
    frame.render_widget(
        panel(
            "Managers",
            format!(
                "Translator: {}\nData Island Manager: {}",
                data.integrations.translator, data.integrations.manager
            ),
        ),
        chunks[0],
    );
    let needle = state.filter.to_ascii_lowercase();
    let sessions: Vec<_> = data
        .integrations
        .sessions
        .iter()
        .filter(|session| session.0.to_ascii_lowercase().contains(&needle))
        .collect();
    let rows = sessions.iter().map(|(id, status, size)| {
        Row::new(vec![
            Cell::from(id.clone()),
            Cell::from(status.clone()),
            Cell::from(size.clone()),
        ])
        .style(state_style(status))
    });
    render_table(
        frame,
        chunks[1],
        "DALiuGE sessions",
        ["Session", "State", "Graph size"],
        rows,
        &[
            Constraint::Percentage(60),
            Constraint::Length(16),
            Constraint::Percentage(25),
        ],
        state.selected[View::Daliuge.index()],
    );
}

fn render_logs(frame: &mut Frame<'_>, area: Rect, state: &ConsoleState, data: &ConsoleData) {
    let needle = state.filter.to_ascii_lowercase();
    let events: Vec<_> = data
        .events
        .iter()
        .filter(|event| {
            [
                event.event_type.as_str(),
                event.project_module.as_str(),
                event.source_identifier.as_deref().unwrap_or(""),
                event.correlation_id.as_deref().unwrap_or(""),
            ]
            .iter()
            .any(|value| value.to_ascii_lowercase().contains(&needle))
        })
        .collect();
    let rows = events.iter().map(|event| {
        Row::new(vec![
            Cell::from(event.occurred_at.format("%m-%d %H:%M:%S").to_string()),
            Cell::from(event.event_type.clone()),
            Cell::from(event.project_module.clone()),
            Cell::from(
                event
                    .source_identifier
                    .clone()
                    .unwrap_or_else(|| "-".into()),
            ),
            Cell::from(event.correlation_id.clone().unwrap_or_else(|| "-".into())),
        ])
    });
    render_table(
        frame,
        area,
        "Structured events (/ filters, p pauses)",
        ["Time", "Event", "Project", "Source", "Correlation"],
        rows,
        &[
            Constraint::Length(16),
            Constraint::Percentage(25),
            Constraint::Percentage(18),
            Constraint::Percentage(22),
            Constraint::Percentage(25),
        ],
        state.selected[View::Logs.index()],
    );
}

fn render_narrow(frame: &mut Frame<'_>, area: Rect, state: &ConsoleState, data: &ConsoleData) {
    let text = match state.view {
        View::Overview => data.overview.as_ref().map_or_else(
            || "Waiting for data".into(),
            |overview| {
                format!(
                    "sources {}\npending {}\nrunning {}\nfailed {}\nqueue {}\nworkers {}",
                    overview.registered_sources,
                    overview.pending_admissions,
                    overview.running_executions,
                    overview.failed_executions,
                    overview.queue_depth,
                    overview.active_workers
                )
            },
        ),
        _ => format!(
            "{} view\nTerminal is narrow; resize to 70 columns for the full table.",
            state.view.title()
        ),
    };
    frame.render_widget(panel(state.view.title(), text), area);
}

fn detail_text(state: &ConsoleState, data: &ConsoleData) -> String {
    match state.view {
        View::Overview => format!(
            "Translator: {}\nData Island Manager: {}\nScheduler: {}\nWorker pools: {}",
            data.integrations.translator,
            data.integrations.manager,
            data.integrations.scheduler,
            data.pools
                .iter()
                .map(|pool| pool.pool.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
        View::Sources => filtered_sources(data, &state.filter)
            .get(state.selected[View::Sources.index()])
            .map(|source| {
                format!(
                    "Source ID: {}\nIdentifier: {}\nProject: {}\nReadiness: {}\nDiscovery signature: {}\nLast executed signature: {}\nLast checked: {}\nWorkflow pending: {}",
                    source.uuid,
                    source.source_identifier,
                    source.project_module,
                    source_state(source),
                    source.discovery_signature.as_deref().unwrap_or("-"),
                    source.last_executed_discovery_signature.as_deref().unwrap_or("-"),
                    format_time(source.last_checked_at),
                    source.workflow_run_pending,
                )
            })
            .unwrap_or_else(|| "No source selected".into()),
        View::Executions => filtered_executions(data, &state.filter)
            .get(state.selected[View::Executions.index()])
            .map(|execution| {
                format!(
                    "Execution ID: {}\nProject: {}\nStatus: {}\nControl phase: {}\nSubmission: {}\nScheduler: {} / {}\nScheduler job ID: {}\nDALiuGE: {}\nDALiuGE session ID: {}\nOutputs: {}\nProfile revision: {}\nDiscovery signature: {}\nManifest SHA-256: {}\nSource graph SHA-256: {}\nPatched graph SHA-256: {}\nPhysical graph SHA-256: {}\nRemote session: {}\nFailure: {}\nLast error: {}",
                    execution.uuid,
                    execution.project_module,
                    execution.status,
                    execution.control_phase.as_deref().unwrap_or("-"),
                    execution.submission_state.as_deref().unwrap_or("-"),
                    execution.scheduler_name.as_deref().unwrap_or("-"),
                    execution.scheduler_state.as_deref().unwrap_or("-"),
                    execution.scheduler_job_id.as_deref().unwrap_or("-"),
                    execution.daliuge_state.as_deref().unwrap_or("-"),
                    execution.daliuge_session_id.as_deref().unwrap_or("-"),
                    execution.output_state.as_deref().unwrap_or("-"),
                    execution.deployment_profile_revision.map(|value| value.to_string()).as_deref().unwrap_or("-"),
                    execution.discovery_signature.as_deref().unwrap_or("-"),
                    execution.manifest_sha256.as_deref().unwrap_or("-"),
                    execution.source_graph_sha256.as_deref().unwrap_or("-"),
                    execution.patched_graph_sha256.as_deref().unwrap_or("-"),
                    execution.physical_graph_sha256.as_deref().unwrap_or("-"),
                    execution.remote_session_dir.as_deref().unwrap_or("-"),
                    execution.failure_class.as_deref().unwrap_or("-"),
                    bounded(execution.last_error.as_deref().unwrap_or("-"), 800),
                )
            })
            .unwrap_or_else(|| "No execution selected".into()),
        View::Workers => filtered_workers(data, &state.filter)
            .get(state.selected[View::Workers.index()])
            .map(|worker| {
                format!(
                    "Worker ID: {}\nInstance: {}\nHost: {}\nRole: {}\nPool: {}\nState: {}\nConcurrency: {}\nStarted: {}\nHeartbeat: {}\nCapabilities: {}\nLabels: {}",
                    worker.uuid,
                    worker.instance_name,
                    worker.host_name,
                    worker.role,
                    worker.pool,
                    worker.status,
                    worker.concurrency_limit,
                    worker.started_at,
                    worker.last_heartbeat_at,
                    worker.capabilities.join(", "),
                    worker.labels,
                )
            })
            .unwrap_or_else(|| "No worker selected".into()),
        View::Scheduler => {
            let needle = state.filter.to_ascii_lowercase();
            data.scheduler_jobs
                .iter()
                .filter(|execution| execution_matches(execution, &needle))
                .nth(state.selected[View::Scheduler.index()])
                .map(|execution| {
                    format!(
                        "Execution ID: {}\nSLURM job ID: {}\nState: {}\nRaw state: {}\nReason: {}\nReconciled: {}\nRemote session: {}",
                        execution.uuid,
                        execution.scheduler_job_id.as_deref().unwrap_or("-"),
                        execution.scheduler_state.as_deref().unwrap_or("-"),
                        execution.scheduler_raw_state.as_deref().unwrap_or("-"),
                        execution.scheduler_reason.as_deref().unwrap_or("-"),
                        format_time(execution.last_reconciled_at),
                        execution.remote_session_dir.as_deref().unwrap_or("-"),
                    )
                })
                .unwrap_or_else(|| "No scheduler job selected".into())
        }
        View::Daliuge => data
            .integrations
            .sessions
            .iter()
            .filter(|session| session.0.to_ascii_lowercase().contains(&state.filter.to_ascii_lowercase()))
            .nth(state.selected[View::Daliuge.index()])
            .map(|session| format!("Session ID: {}\nState: {}\nGraph size: {}", session.0, session.1, session.2))
            .unwrap_or_else(|| "No DALiuGE session selected".into()),
        View::Logs => {
            let needle = state.filter.to_ascii_lowercase();
            data.events
                .iter()
                .filter(|event| {
                    event.event_type.to_ascii_lowercase().contains(&needle)
                        || event.project_module.to_ascii_lowercase().contains(&needle)
                        || event.correlation_id.as_deref().unwrap_or("").to_ascii_lowercase().contains(&needle)
                })
                .nth(state.selected[View::Logs.index()])
                .map(|event| {
                    format!(
                        "Event ID: {}\nOccurred: {}\nType: {}\nProject: {}\nSource: {}\nExecution: {}\nActor: {}\nCorrelation: {}\n\nPayload\n{}",
                        event.id,
                        event.occurred_at,
                        event.event_type,
                        event.project_module,
                        event.source_identifier.as_deref().unwrap_or("-"),
                        event.execution_id.map(|id| id.to_string()).as_deref().unwrap_or("-"),
                        event.actor.as_deref().unwrap_or("-"),
                        event.correlation_id.as_deref().unwrap_or("-"),
                        bounded(&serde_json::to_string_pretty(&event.payload).unwrap_or_default(), 3000),
                    )
                })
                .unwrap_or_else(|| "No event selected".into())
        }
    }
}

fn render_table<'a, const N: usize>(
    frame: &mut Frame<'_>,
    area: Rect,
    title: &str,
    headers: [&str; N],
    rows: impl IntoIterator<Item = Row<'a>>,
    widths: &[Constraint],
    selected: usize,
) {
    let header = Row::new(headers.map(Cell::from))
        .style(Style::default().add_modifier(Modifier::BOLD))
        .bottom_margin(1);
    let rows: Vec<_> = rows.into_iter().collect();
    let selected = selected.min(rows.len().saturating_sub(1));
    let mut table_state = ratatui::widgets::TableState::default().with_selected(Some(selected));
    let table = Table::new(rows, widths)
        .header(header)
        .block(
            Block::default()
                .title(format!(" {title} "))
                .borders(Borders::ALL),
        )
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");
    frame.render_stateful_widget(table, area, &mut table_state);
}

fn panel(title: &str, content: impl Into<String>) -> Paragraph<'static> {
    Paragraph::new(content.into())
        .block(
            Block::default()
                .title(format!(" {title} "))
                .borders(Borders::ALL),
        )
        .wrap(Wrap { trim: false })
}

fn render_popup(frame: &mut Frame<'_>, title: &str, text: &str) {
    let area = centered_rect(62, 34, frame.area());
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(text.to_string())
            .block(
                Block::default()
                    .title(format!(" {title} "))
                    .borders(Borders::ALL),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1])[1]
}

fn filtered_sources<'a>(data: &'a ConsoleData, filter: &str) -> Vec<&'a SourceRegistryRow> {
    let needle = filter.to_ascii_lowercase();
    data.sources
        .iter()
        .filter(|source| {
            source
                .source_identifier
                .to_ascii_lowercase()
                .contains(&needle)
                || source.project_module.to_ascii_lowercase().contains(&needle)
        })
        .collect()
}

fn filtered_executions<'a>(data: &'a ConsoleData, filter: &str) -> Vec<&'a ExecutionRow> {
    let needle = filter.to_ascii_lowercase();
    data.executions
        .iter()
        .filter(|execution| execution_matches(execution, &needle))
        .collect()
}

fn execution_matches(execution: &ExecutionRow, needle: &str) -> bool {
    execution.uuid.to_string().contains(needle)
        || execution
            .project_module
            .to_ascii_lowercase()
            .contains(needle)
        || execution.status.to_ascii_lowercase().contains(needle)
        || execution
            .scheduler_job_id
            .as_deref()
            .unwrap_or("")
            .to_ascii_lowercase()
            .contains(needle)
        || execution
            .daliuge_session_id
            .as_deref()
            .unwrap_or("")
            .to_ascii_lowercase()
            .contains(needle)
}

fn filtered_workers<'a>(data: &'a ConsoleData, filter: &str) -> Vec<&'a WorkerInstanceRow> {
    let needle = filter.to_ascii_lowercase();
    data.workers
        .iter()
        .filter(|worker| {
            worker.uuid.to_string().contains(&needle)
                || worker.instance_name.to_ascii_lowercase().contains(&needle)
                || worker.host_name.to_ascii_lowercase().contains(&needle)
                || worker.pool.to_ascii_lowercase().contains(&needle)
                || worker
                    .capabilities
                    .iter()
                    .any(|capability| capability.to_ascii_lowercase().contains(&needle))
        })
        .collect()
}

fn source_state(source: &SourceRegistryRow) -> String {
    if !source.enabled {
        "disabled".into()
    } else if source.workflow_run_pending {
        "pending admission".into()
    } else if source
        .discovery_claim_expires_at
        .is_some_and(|expiry| expiry > Utc::now())
    {
        "discovering".into()
    } else if source.discovery_signature.is_none() {
        "not discovered".into()
    } else if source.discovery_signature != source.last_executed_discovery_signature {
        "ready (changed)".into()
    } else {
        "executed".into()
    }
}

fn state_style(state: &str) -> Style {
    match state.to_ascii_lowercase().as_str() {
        "failed" | "unhealthy" | "error" => Style::default().fg(Color::Red),
        "running" | "active" | "finished" | "completed" => Style::default().fg(Color::Green),
        "pending" | "awaiting_scheduler" | "draining" | "unknown" => {
            Style::default().fg(Color::Yellow)
        }
        _ => Style::default(),
    }
}

fn short_id(id: Uuid) -> String {
    id.simple().to_string()[..8].to_string()
}

fn short_hash(value: Option<&str>) -> String {
    value
        .map(|value| value.chars().take(10).collect())
        .unwrap_or_else(|| "-".into())
}

fn format_time(value: Option<DateTime<Utc>>) -> String {
    value
        .map(|value| value.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| "-".into())
}

fn bounded(value: &str, limit: usize) -> String {
    value.chars().take(limit).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn view_navigation_wraps_in_both_directions() {
        let mut state = ConsoleState::default();
        state.next_view(true);
        assert_eq!(state.view, View::Logs);
        state.next_view(false);
        assert_eq!(state.view, View::Overview);
    }

    #[test]
    fn selection_never_underflows() {
        let mut state = ConsoleState::default();
        state.move_selection(-1);
        assert_eq!(state.selected[0], 0);
    }

    #[test]
    fn endpoint_filter_matches_external_identifiers() {
        let mut execution: ExecutionRow = serde_json::from_value(serde_json::json!({
            "uuid": Uuid::nil(),
            "project_module": "wallaby_hires",
            "sources": [],
            "archive_name": "casda",
            "deployment_profile_id": null,
            "deployment_profile_revision": null,
            "deployment_profile_snapshot": null,
            "project_config_id": null,
            "discovery_signature": null,
            "workflow_manifest": null,
            "manifest_sha256": null,
            "source_graph_sha256": null,
            "patched_graph_sha256": null,
            "physical_graph_sha256": null,
            "execution_phase": null,
            "control_phase": "submitted",
            "submission_state": "submitted",
            "scheduler_name": "slurm",
            "scheduler_job_id": "12345",
            "scheduler_state": "running",
            "scheduler_raw_state": "RUNNING",
            "scheduler_reason": null,
            "daliuge_session_id": "BeampipeExecution-test",
            "daliuge_manager_url": null,
            "daliuge_state": "running",
            "daliuge_raw_status": null,
            "output_state": "pending",
            "output_verification_required": false,
            "remote_session_dir": null,
            "terminal_outcome": null,
            "failure_class": null,
            "phase_timestamps": {},
            "last_reconciled_at": null,
            "last_error": null,
            "created_by_id": null,
            "status": "running",
            "retry_count": 0,
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": null,
            "started_at": null,
            "completed_at": null
        }))
        .unwrap();
        assert!(execution_matches(&execution, "12345"));
        execution.scheduler_job_id = None;
        assert!(execution_matches(&execution, "beampipeexecution"));
    }
}
