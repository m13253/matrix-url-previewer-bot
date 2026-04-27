use std::path::PathBuf;
use std::sync::Arc;

use eyre::Result;
use indexmap::IndexSet;
use matrix_sdk::config::SyncSettings;
use matrix_sdk::event_handler::{Ctx, RawEvent};
use matrix_sdk::ruma::api::client::filter::FilterDefinition;
use matrix_sdk::ruma::events::room::encrypted::OriginalSyncRoomEncryptedEvent;
use matrix_sdk::ruma::events::room::member::{MembershipState, SyncRoomMemberEvent};
use matrix_sdk::ruma::events::room::message::{
    MessageFormat, MessageType, OriginalSyncRoomMessageEvent, Relation,
};
use matrix_sdk::ruma::events::room::redaction::OriginalSyncRoomRedactionEvent;
use matrix_sdk::{Client, Room, RoomState};
use tracing::{Instrument, error, info, instrument, warn};
use tracing_subscriber::{EnvFilter, prelude::*};
use url::Url;

use crate::worker::Worker;

mod common;
mod config;
mod extract_url;
mod html_escape;
mod limit;
mod worker;

#[derive(clap::Parser)]
struct Args {
    #[clap(subcommand)]
    command: Command,
}

#[derive(clap::Subcommand)]
enum Command {
    #[clap(about = "Perform initial setup of Matrix account")]
    Setup {
        #[clap(
            long = "config",
            value_name = "PATH",
            help = "Path to the configuration file"
        )]
        config_path: PathBuf,
        #[clap(
            long,
            value_name = "DEVICE_NAME",
            default_value = concat!("matrixbot-ezlogin/", env!("CARGO_BIN_NAME")),
            help = "Device name to use for this session"
        )]
        device_name: String,
    },
    #[clap(about = "Run the bot")]
    Run {
        #[clap(
            long = "config",
            value_name = "PATH",
            help = "Path to the configuration file"
        )]
        config_path: PathBuf,
    },
    #[clap(about = "Log out of the Matrix session, and delete the state database")]
    Logout {
        #[clap(
            long = "config",
            value_name = "PATH",
            help = "Path to the configuration file"
        )]
        config_path: PathBuf,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install()?;
    matrixbot_ezlogin::DuplexLog::init();
    tracing_subscriber::registry()
        .with(tracing_error::ErrorLayer::default())
        .with({
            let mut filter = EnvFilter::new(concat!(
                "warn,",
                env!("CARGO_CRATE_NAME"),
                "=debug,matrixbot_ezlogin=info"
            ));
            if let Some(env) = std::env::var_os(EnvFilter::DEFAULT_ENV) {
                for segment in env.to_string_lossy().split(',') {
                    if let Ok(directive) = segment.parse() {
                        filter = filter.add_directive(directive);
                    }
                }
            }
            filter
        })
        .with(
            tracing_subscriber::fmt::layer().with_writer(matrixbot_ezlogin::DuplexLog::get_writer),
        )
        .init();

    let args: Args = clap::Parser::parse();

    match args.command {
        Command::Setup {
            config_path,
            device_name,
        } => {
            let config = config::Config::new(&config_path).await?;
            drop(matrixbot_ezlogin::setup_interactive(&config.data_dir, &device_name).await?);
        }
        Command::Run { config_path } => {
            let config = config::Config::new(&config_path).await?;
            run(config).await?;
        }
        Command::Logout { config_path } => {
            let config = config::Config::new(&config_path).await?;
            matrixbot_ezlogin::logout(&config.data_dir).await?
        }
    };
    Ok(())
}

async fn run(config: Arc<config::Config>) -> Result<()> {
    let worker = Worker::new(config.clone()).await?;
    let (client, sync_helper) = matrixbot_ezlogin::login(&config.data_dir).await?;

    // We don't ignore joining and leaving events happened during downtime.
    client.add_event_handler_context(worker);
    client.add_event_handler(on_leave);

    // Enable room members lazy-loading, it will speed up the initial sync a lot with accounts in lots of rooms.
    // https://spec.matrix.org/v1.6/client-server-api/#lazy-loading-room-members
    let sync_settings =
        SyncSettings::default().filter(FilterDefinition::with_lazy_loading().into());

    info!(
        "Skipping messages since last logout. May take longer depending on the number of rooms joined."
    );
    sync_helper
        .sync_once(&client, sync_settings.clone())
        .await?;

    client.add_event_handler(on_message);
    client.add_event_handler(on_deletion);
    client.add_event_handler(on_utd);

    // Forget rooms that we already left
    let left_rooms = client.left_rooms();
    tokio::spawn(
        async move {
            for room in left_rooms {
                info!("Forgetting room {}.", room.room_id());
                match room.forget().await {
                    Ok(_) => info!("Forgot room {}.", room.room_id()),
                    Err(err) => error!("Failed to forget room {}: {}", room.room_id(), err),
                }
            }
        }
        .in_current_span(),
    );

    info!("Starting sync.");
    sync_helper.sync(&client, sync_settings).await?;

    Ok(())
}

// https://spec.matrix.org/v1.14/client-server-api/#mroommessage
#[instrument(skip_all)]
async fn on_message(
    event: OriginalSyncRoomMessageEvent,
    room: Room,
    client: Client,
    ctx: Ctx<Arc<Worker>>,
) -> Result<()> {
    if event.sender == client.user_id().unwrap() {
        // Ignore my own message
        return Ok(());
    }
    if room.state() != RoomState::Joined {
        info!(
            "Ignoring room {}: Current room state is {:?}.",
            room.room_id(),
            room.state()
        );
        return Ok(());
    }

    let (original_event_id, thread_id, latest_content) = match event.content.relates_to {
        Some(Relation::Replacement(replacement)) => {
            (replacement.event_id, None, replacement.new_content)
        }
        Some(Relation::Thread(ref thread)) => (
            event.event_id,
            Some(thread.event_id.clone()),
            event.content.into(),
        ),
        _ => (event.event_id, None, event.content.into()),
    };
    let MessageType::Text(text) = latest_content.msgtype else {
        return Ok(());
    };
    let html = text
        .formatted
        .filter(|formatted| formatted.format == MessageFormat::Html);
    let urls = if let Some(html) = html {
        extract_url::extract_urls_from_html(&html.body)
    } else {
        // This code causes Internal Compiler Error on Rustc 1.87.0:
        // text.body
        //     .lines()
        //     .skip_while(|&line| line.starts_with("> "))
        //     .flat_map(extract_url::extract_urls_from_text)
        //     .collect::<IndexSet<Url>>()
        text.body
            .lines()
            .skip_while(|&line| line.starts_with("> "))
            .flat_map(|line| extract_url::extract_urls_from_text(line))
            .collect::<IndexSet<Url>>()
    };

    ctx.0
        .on_message(room, thread_id, original_event_id, urls)
        .await?;
    Ok(())
}

#[instrument(skip_all)]
async fn on_deletion(
    event: OriginalSyncRoomRedactionEvent,
    room: Room,
    client: Client,
    ctx: Ctx<Arc<Worker>>,
) -> Result<()> {
    if event.sender == client.user_id().unwrap() {
        // Ignore my own message
        return Ok(());
    }
    if room.state() != RoomState::Joined {
        info!(
            "Ignoring room {}: Current room state is {:?}.",
            room.room_id(),
            room.state()
        );
        return Ok(());
    }

    let room_version_rules = room.clone_info().room_version_rules_or_default();
    let original_event_id = event.redacts(&room_version_rules.redaction);
    ctx.0.on_deletion(room, &original_event_id).await?;
    Ok(())
}

// https://spec.matrix.org/v1.14/client-server-api/#mroomencrypted
#[instrument(skip_all)]
async fn on_utd(_event: OriginalSyncRoomEncryptedEvent, room: Room, raw_event: RawEvent) {
    error!(
        "Unable to decrypt: room {}, event {}",
        room.room_id(),
        raw_event.get()
    );
}

// https://spec.matrix.org/v1.14/client-server-api/#mroommember
#[instrument(skip_all)]
async fn on_leave(event: SyncRoomMemberEvent, room: Room) {
    if !matches!(
        event.membership(),
        MembershipState::Leave | MembershipState::Ban
    ) {
        return;
    }

    match room.state() {
        RoomState::Joined => {
            tokio::spawn(
                async move {
                    if let Err(err) = room.sync_members().await {
                        warn!("Failed to sync members of {}: {}", room.room_id(), err);
                    }
                    // Only I remain in the room.
                    if room.joined_members_count() <= 1 {
                        info!("Leaving room {}.", room.room_id());
                        match room.leave().await {
                            Ok(_) => info!("Left room {}.", room.room_id()),
                            Err(err) => {
                                error!("Failed to leave room {}: {}", room.room_id(), err)
                            }
                        }
                    }
                }
                .in_current_span(),
            );
        }
        RoomState::Banned | RoomState::Left => {
            // Either I successfully left the room, or someone kicked me out.
            tokio::spawn(
                async move {
                    info!("Forgetting room {}.", room.room_id());
                    match room.forget().await {
                        Ok(_) => info!("Forgot room {}.", room.room_id()),
                        Err(err) => error!("Failed to forget room {}: {}", room.room_id(), err),
                    }
                }
                .in_current_span(),
            );
        }
        _ => (),
    }
}
