// SPDX-FileCopyrightText: 2026 The Rundenzeit Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `rz-server`: runs the server, or administers accounts from the command
//! line (for the very first admin, or when every admin is locked out).

use std::net::SocketAddr;

use clap::{Parser, Subcommand};
use rz_server::config::Config;
use rz_server::state::audit;
use rz_server::{admin, auth, crypto};

#[derive(Debug, Parser)]
#[command(name = "rz-server", version, about = "Rundenzeit server")]
struct Cli {
    #[command(flatten)]
    config: Config,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run the server (default)
    Serve,
    /// Account administration
    #[command(subcommand)]
    Admin(AdminCommand),
}

#[derive(Debug, Subcommand)]
enum AdminCommand {
    /// Create an account with a temporary password
    CreateUser {
        /// Login name
        username: String,
        /// Make the account a server admin
        #[arg(long)]
        admin: bool,
    },
    /// Set a new temporary password, remove 2FA and end all sessions
    ResetUser {
        /// Login name
        username: String,
        /// Also make the account an admin and enable it
        #[arg(long)]
        admin: bool,
    },
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,tower_http=info".into()),
        )
        .init();
    let cli = Cli::parse();
    let result = match cli.command.unwrap_or(Command::Serve) {
        Command::Serve => serve(cli.config).await,
        Command::Admin(cmd) => run_admin(cli.config, cmd).await,
    };
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}

async fn serve(config: Config) -> Result<(), Box<dyn std::error::Error>> {
    let listen = config.listen;
    let state = rz_server::init(config, None).await?;
    let app = rz_server::app(state);
    let listener = tokio::net::TcpListener::bind(listen).await?;
    tracing::info!(address = %listen, "rz-server listening");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async {
        let _ = tokio::signal::ctrl_c().await;
        tracing::info!("shutting down");
    })
    .await?;
    Ok(())
}

async fn run_admin(config: Config, cmd: AdminCommand) -> Result<(), Box<dyn std::error::Error>> {
    let state = rz_server::init(config, None).await?;
    let mut db = state.conn().await?;
    match cmd {
        AdminCommand::CreateUser { username, admin } => {
            let username = auth::normalize_username(&username)?;
            let password = crypto::temporary_password();
            let hash = crypto::hash_password(password.clone()).await?;
            let id = auth::create_user(&db, &username, "", &hash, admin, true).await?;
            audit(
                &db,
                Some(id),
                "cli_user_created",
                None,
                serde_json::json!({ "admin": admin }),
            )
            .await;
            println!("created {username}; temporary password (must be changed at first login):");
            println!("{password}");
        }
        AdminCommand::ResetUser { username, admin } => {
            let username = auth::normalize_username(&username)?;
            let tx = db.transaction().await?;
            let row = tx
                .query_opt("SELECT id FROM users WHERE username = $1", &[&username])
                .await?
                .ok_or("no such user")?;
            let id: uuid::Uuid = row.get(0);
            let password = admin::reset_user(&tx, id, true, true).await?;
            if admin {
                tx.execute(
                    "UPDATE users SET is_admin = true, disabled = false WHERE id = $1",
                    &[&id],
                )
                .await?;
            }
            tx.commit().await?;
            audit(
                &db,
                Some(id),
                "cli_user_reset",
                None,
                serde_json::json!({ "admin": admin }),
            )
            .await;
            println!("reset {username}: two-factor authentication removed, sessions ended.");
            println!("temporary password (must be changed at first login):");
            println!("{}", password.unwrap_or_default());
        }
    }
    Ok(())
}
