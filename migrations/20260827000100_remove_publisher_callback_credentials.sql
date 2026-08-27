-- Remote Slurm publication is reconciled by Core pulling the immutable
-- inventory receipt over its existing authenticated SSH channel. Publisher
-- callback capabilities are no longer issued or accepted.
DROP TABLE IF EXISTS execution_publisher_credentials;
