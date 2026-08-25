ALTER TABLE execution_publisher_credentials
    ADD CONSTRAINT execution_publisher_credentials_lifetime_bounded CHECK (
        expires_at > created_at
        AND expires_at <= created_at + INTERVAL '24 hours'
    );
