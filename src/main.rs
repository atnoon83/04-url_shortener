use std::io::stdout;
use std::net::SocketAddr;
use anyhow::Result;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse};
use axum::{Json, Router};
use axum::routing::{get, post};
use sqlx::{FromRow, PgPool, query, query_as};
use tracing::{info, instrument, warn};
use tracing::level_filters::LevelFilter;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{fmt, Layer};
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::fmt::writer::MakeWriterExt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
struct AppState {
    db: PgPool,
}

#[derive(Debug, Deserialize)]
struct ShortenRequest {
    url: String,
}

#[derive(Debug, Serialize)]
struct ShortenResponse {
    url: String,
}

#[derive(Debug, FromRow)]
struct UrlResult {
    #[sqlx(default)]
    id: String,
    #[sqlx(default)]
    url: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let _guard = init_tracing();

    let addr = SocketAddr::from(([127, 0, 0, 1], 9527));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!("Listening on: {}", addr);

    let state = AppState::try_new("postgres://postgres:postgres@localhost/shortener")
        .await?;

    let app = Router::new()
        .route("/", post(shorten_url_handler))
        .route("/:id", get(get_url_handler))
        .with_state(state);

    axum::serve(listener, app.into_make_service()).await?;

    Ok(())
}

fn init_tracing() -> WorkerGuard {
    let file_appender = tracing_appender::rolling::never(".", "app.log");
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

    let writer = non_blocking.and(stdout);

    let layer = fmt::Layer::new()
        .with_span_events(FmtSpan::ENTER | FmtSpan::CLOSE)
        .with_writer(writer)
        .with_level(true)
        .with_filter(LevelFilter::TRACE);

    tracing_subscriber::registry().with(layer).init();

    guard
}

#[instrument]
async fn shorten_url_handler(
    State(state): State<AppState>,
    Json(body): Json<ShortenRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    &body.url.parse::<Uri>().map_err(|_| {
        warn!("Invalid URL: {}", body.url);
        StatusCode::UNPROCESSABLE_ENTITY
    })?;
    let id = state.shorten_url(&body.url).await
        .map_err(|_| {
            warn!("Failed to shorten URL: {}", body.url);
            StatusCode::UNPROCESSABLE_ENTITY
        })?;
    let body = Json(
        ShortenResponse {
            url: format!("localhost:9527/{}", id),
        }
    );

    Ok((StatusCode::CREATED, body))
}

#[instrument]
async fn get_url_handler(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, StatusCode> {
    let url = state.get_url(&id).await.map_err(|_| {
        warn!("Failed to get URL for ID: {:?}", id);
        StatusCode::NOT_FOUND
    })?;
    let mut headers = HeaderMap::new();
    headers.insert("Location", url.parse().unwrap());
    Ok((StatusCode::PERMANENT_REDIRECT, headers))
}

impl AppState {
    async fn try_new(db_url: &str) -> Result<Self> {
        let db = PgPool::connect(db_url).await?;
        query(
            r#"
            CREATE TABLE IF NOT EXISTS urls (
                id CHAR(6) PRIMARY KEY,
                url TEXT NOT NULL UNIQUE
            )
            "#
        ).execute(&db).await?;

        Ok(Self { db })
    }

    async fn shorten_url(&self, url: &str) -> Result<String> {
        let id = nanoid::nanoid!(6);
        let res: UrlResult = query_as(
            r#"
            INSERT INTO urls (id, url) VALUES ($1, $2)
            RETURNING id, url
            "#
        ).bind(id).bind(url)
            .fetch_one(&self.db)
            .await?;

        Ok(res.id)
    }

    async fn get_url(&self, id: &str) -> Result<String> {
        let res: UrlResult = query_as(
            r#"
            SELECT id, url FROM urls WHERE id = $1
            "#
        ).bind(id)
            .fetch_one(&self.db)
            .await?;

        Ok(res.url)
    }
}