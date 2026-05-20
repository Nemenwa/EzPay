use anyhow::{Context, Result};
use deadpool_postgres::{Config, Pool, Runtime};
use std::time::Duration;
use tokio_postgres::NoTls;
use tracing::{info, warn, error};

/// Database configuration loaded from environment variables
#[derive(Debug, Clone)]
pub struct DatabaseConfig {
    pub database_url: String,
    pub max_connections: u32,
    pub min_connections: u32,
    pub connection_timeout: Duration,
    pub idle_timeout: Duration,
    pub max_lifetime: Duration,
    pub query_timeout: Duration,
    pub retry_attempts: u32,
    pub retry_delay: Duration,
}

impl DatabaseConfig {
    /// Load configuration from environment variables
    pub fn from_env() -> Result<Self> {
        let database_url = std::env::var("DATABASE_URL")
            .context("DATABASE_URL environment variable must be set")?;

        let max_connections = std::env::var("DB_MAX_CONNECTIONS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(10);

        let min_connections = std::env::var("DB_MIN_CONNECTIONS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(2);

        let connection_timeout_secs = std::env::var("DB_CONNECTION_TIMEOUT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(30);

        let idle_timeout_secs = std::env::var("DB_IDLE_TIMEOUT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(600);

        let max_lifetime_secs = std::env::var("DB_MAX_LIFETIME")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(1800);

        let query_timeout_secs = std::env::var("DB_QUERY_TIMEOUT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(30);

        let retry_attempts = std::env::var("DB_RETRY_ATTEMPTS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(5);

        let retry_delay_secs = std::env::var("DB_RETRY_DELAY")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(2);

        Ok(Self {
            database_url,
            max_connections,
            min_connections,
            connection_timeout: Duration::from_secs(connection_timeout_secs),
            idle_timeout: Duration::from_secs(idle_timeout_secs),
            max_lifetime: Duration::from_secs(max_lifetime_secs),
            query_timeout: Duration::from_secs(query_timeout_secs),
            retry_attempts,
            retry_delay: Duration::from_secs(retry_delay_secs),
        })
    }
}

/// Database connection pool with retry logic and health checks
pub struct Database {
    pool: Pool,
    sqlx_pool: sqlx::PgPool,
    config: DatabaseConfig,
}

impl Database {
    /// Create a new database connection pool with retry logic
    pub async fn new(config: DatabaseConfig) -> Result<Self> {
        let mut attempt = 0;
        let max_attempts = config.retry_attempts;

        loop {
            attempt += 1;
            
            match Self::create_pool(&config).await {
                Ok(pool) => {
                    let sqlx_pool = sqlx::postgres::PgPoolOptions::new()
                        .max_connections(config.max_connections)
                        .connect(&config.database_url)
                        .await
                        .context("Failed to connect to database using sqlx")?;

                    info!("Database connection pool established successfully");
                    return Ok(Self { pool, sqlx_pool, config });
                }
                Err(e) => {
                    if attempt >= max_attempts {
                        error!(
                            "Failed to establish database connection after {} attempts: {}",
                            max_attempts, e
                        );
                        return Err(e).context("Failed to establish database connection after retries");
                    }
                    
                    warn!(
                        "Database connection attempt {} failed: {}. Retrying in {:?}...",
                        attempt, e, config.retry_delay
                    );
                    tokio::time::sleep(config.retry_delay).await;
                }
            }
        }
    }

    /// Create the connection pool
    async fn create_pool(config: &DatabaseConfig) -> Result<Pool> {
        let mut pg_config = Config::new();
        
        // Parse the database URL
        pg_config.url = Some(config.database_url.clone());
        
        // Configure pool settings
        pg_config.manager = Some(deadpool_postgres::ManagerConfig {
            recycling_method: deadpool_postgres::RecyclingMethod::Fast,
        });

        // Configure pool size
        pg_config.pool = Some(deadpool::managed::PoolConfig {
            max_size: config.max_connections as usize,
            timeouts: deadpool::managed::Timeouts {
                wait: Some(config.connection_timeout),
                create: Some(config.connection_timeout),
                recycle: Some(config.idle_timeout),
            },
            queue_mode: deadpool::managed::QueueMode::Fifo,
        });

        let pool = pg_config
            .create_pool(Some(Runtime::Tokio1), NoTls)
            .context("Failed to create connection pool")?;

        Ok(pool)
    }

    /// Get a connection from the pool
    pub async fn get_connection(&self) -> Result<deadpool_postgres::Client> {
        self.pool
            .get()
            .await
            .context("Failed to get database connection from pool")
    }

    /// Get the connection pool
    pub fn pool(&self) -> &Pool {
        &self.pool
    }

    /// Get the sqlx connection pool
    pub fn sqlx_pool(&self) -> &sqlx::PgPool {
        &self.sqlx_pool
    }

    /// Get the database configuration
    pub fn config(&self) -> &DatabaseConfig {
        &self.config
    }

    /// Health check - verify database connection is working
    pub async fn health_check(&self) -> DatabaseHealth {
        let start = std::time::Instant::now();
        
        match self.get_connection().await {
            Ok(client) => {
                match client.query_one("SELECT 1", &[]).await {
                    Ok(_) => {
                        let latency = start.elapsed();
                        let pool_status = self.pool.status();
                        
                        DatabaseHealth {
                            healthy: true,
                            message: "Database connection successful".to_string(),
                            latency_ms: latency.as_millis() as u64,
                            pool_size: pool_status.size,
                            pool_available: pool_status.available,
                            max_connections: self.config.max_connections,
                        }
                    }
                    Err(e) => {
                        error!("Database health check query failed: {}", e);
                        DatabaseHealth {
                            healthy: false,
                            message: format!("Database query failed: {}", e),
                            latency_ms: start.elapsed().as_millis() as u64,
                            pool_size: 0,
                            pool_available: 0,
                            max_connections: self.config.max_connections,
                        }
                    }
                }
            }
            Err(e) => {
                error!("Failed to get connection for health check: {}", e);
                DatabaseHealth {
                    healthy: false,
                    message: format!("Failed to get database connection: {}", e),
                    latency_ms: start.elapsed().as_millis() as u64,
                    pool_size: 0,
                    pool_available: 0,
                    max_connections: self.config.max_connections,
                }
            }
        }
    }

    /// Execute a query with timeout
    pub async fn query_with_timeout<T, F>(
        &self,
        operation: F,
    ) -> Result<T>
    where
        F: std::future::Future<Output = Result<T>>,
    {
        tokio::time::timeout(self.config.query_timeout, operation)
            .await
            .context("Query timed out")?
    }

    /// Close the connection pool
    pub async fn close(&self) {
        info!("Closing database connection pool");
        self.pool.close();
    }
}

impl Clone for Database {
    fn clone(&self) -> Self {
        Self {
            pool: self.pool.clone(),
            sqlx_pool: self.sqlx_pool.clone(),
            config: self.config.clone(),
        }
    }
}

/// Database health status
#[derive(Debug, serde::Serialize)]
pub struct DatabaseHealth {
    pub healthy: bool,
    pub message: String,
    pub latency_ms: u64,
    pub pool_size: usize,
    pub pool_available: usize,
    pub max_connections: u32,
}

/// Health check endpoint handler
pub async fn health_check_handler(
    axum::extract::State(database): axum::extract::State<Database>,
) -> axum::Json<DatabaseHealth> {
    let health = database.health_check().await;
    axum::Json(health)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set_test_env(vars: Vec<(&str, &str)>) {
        for (key, value) in vars {
            std::env::set_var(key, value);
        }
    }

    fn remove_test_env(vars: Vec<&str>) {
        for key in vars {
            std::env::remove_var(key);
        }
    }

    #[test]
    fn test_config_from_env() {
        set_test_env(vec![
            ("DATABASE_URL", "postgresql://localhost/test"),
            ("DB_MAX_CONNECTIONS", "20"),
            ("DB_QUERY_TIMEOUT", "60"),
        ]);

        let config = DatabaseConfig::from_env().unwrap();
        
        assert_eq!(config.database_url, "postgresql://localhost/test");
        assert_eq!(config.max_connections, 20);
        assert_eq!(config.query_timeout, Duration::from_secs(60));

        remove_test_env(vec![
            "DATABASE_URL",
            "DB_MAX_CONNECTIONS",
            "DB_QUERY_TIMEOUT",
        ]);
    }

    #[test]
    fn test_config_defaults() {
        set_test_env(vec![
            ("DATABASE_URL", "postgresql://localhost/test"),
        ]);

        let config = DatabaseConfig::from_env().unwrap();
        
        assert_eq!(config.max_connections, 10);
        assert_eq!(config.min_connections, 2);
        assert_eq!(config.connection_timeout, Duration::from_secs(30));
        assert_eq!(config.query_timeout, Duration::from_secs(30));
        assert_eq!(config.retry_attempts, 5);

        remove_test_env(vec!["DATABASE_URL"]);
    }

    #[test]
    fn test_config_missing_database_url() {
        remove_test_env(vec!["DATABASE_URL"]);

        let result = DatabaseConfig::from_env();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("DATABASE_URL"));
    }

    #[test]
    fn test_config_all_environment_variables() {
        set_test_env(vec![
            ("DATABASE_URL", "postgresql://user:pass@localhost:5432/mydb"),
            ("DB_MAX_CONNECTIONS", "50"),
            ("DB_MIN_CONNECTIONS", "5"),
            ("DB_CONNECTION_TIMEOUT", "60"),
            ("DB_IDLE_TIMEOUT", "300"),
            ("DB_MAX_LIFETIME", "3600"),
            ("DB_QUERY_TIMEOUT", "45"),
            ("DB_RETRY_ATTEMPTS", "10"),
            ("DB_RETRY_DELAY", "5"),
        ]);

        let config = DatabaseConfig::from_env().unwrap();
        
        assert_eq!(config.database_url, "postgresql://user:pass@localhost:5432/mydb");
        assert_eq!(config.max_connections, 50);
        assert_eq!(config.min_connections, 5);
        assert_eq!(config.connection_timeout, Duration::from_secs(60));
        assert_eq!(config.idle_timeout, Duration::from_secs(300));
        assert_eq!(config.max_lifetime, Duration::from_secs(3600));
        assert_eq!(config.query_timeout, Duration::from_secs(45));
        assert_eq!(config.retry_attempts, 10);
        assert_eq!(config.retry_delay, Duration::from_secs(5));

        remove_test_env(vec![
            "DATABASE_URL",
            "DB_MAX_CONNECTIONS",
            "DB_MIN_CONNECTIONS",
            "DB_CONNECTION_TIMEOUT",
            "DB_IDLE_TIMEOUT",
            "DB_MAX_LIFETIME",
            "DB_QUERY_TIMEOUT",
            "DB_RETRY_ATTEMPTS",
            "DB_RETRY_DELAY",
        ]);
    }

    #[test]
    fn test_config_invalid_values_use_defaults() {
        set_test_env(vec![
            ("DATABASE_URL", "postgresql://localhost/test"),
            ("DB_MAX_CONNECTIONS", "invalid"),
            ("DB_QUERY_TIMEOUT", "not_a_number"),
        ]);

        let config = DatabaseConfig::from_env().unwrap();
        
        // Invalid values should fall back to defaults
        assert_eq!(config.max_connections, 10);
        assert_eq!(config.query_timeout, Duration::from_secs(30));

        remove_test_env(vec![
            "DATABASE_URL",
            "DB_MAX_CONNECTIONS",
            "DB_QUERY_TIMEOUT",
        ]);
    }

    #[test]
    fn test_database_health_serialization() {
        let health = DatabaseHealth {
            healthy: true,
            message: "Test message".to_string(),
            latency_ms: 100,
            pool_size: 10,
            pool_available: 5,
            max_connections: 20,
        };

        let json = serde_json::to_string(&health).unwrap();
        assert!(json.contains("\"healthy\":true"));
        assert!(json.contains("\"message\":\"Test message\""));
        assert!(json.contains("\"latency_ms\":100"));
        assert!(json.contains("\"pool_size\":10"));
        assert!(json.contains("\"pool_available\":5"));
        assert!(json.contains("\"max_connections\":20"));
    }

    #[test]
    fn test_database_config_clone() {
        set_test_env(vec![
            ("DATABASE_URL", "postgresql://localhost/test"),
        ]);
        
        let config = DatabaseConfig::from_env().unwrap();
        let cloned = config.clone();
        
        assert_eq!(config.database_url, cloned.database_url);
        assert_eq!(config.max_connections, cloned.max_connections);

        remove_test_env(vec!["DATABASE_URL"]);
    }
}
