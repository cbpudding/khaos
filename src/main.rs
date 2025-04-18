use deadpool_redis::{redis, Config, Connection, Runtime};
use std::time::{Duration, SystemTime, SystemTimeError};
use std::{error::Error, fs, sync::Arc};
use twilight_cache_inmemory::DefaultInMemoryCache;
use twilight_gateway::{Event, EventTypeFlags, Intents, Shard, ShardId, StreamExt};
use twilight_http::{request::channel::reaction::RequestReactionType, Client};
use twilight_model::{
    channel::Message,
    id::{
        marker::{ChannelMarker, MessageMarker, UserMarker},
        Id,
    },
};

const SUCCESS_REACTION: RequestReactionType = RequestReactionType::Unicode { name: "✅" };

mod config;

use config::KhaosControl;

// TODO: Create a poll every two weeks to determine the new leader of the server
// TODO: Prevent the leader from fighting the bot

async fn handle_event(
    config: KhaosControl,
    http: Arc<Client>,
    database: Connection,
    event: Event,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    match event {
        Event::GatewayHeartbeatAck => {}
        Event::GatewayHello(_) => {}
        Event::GuildCreate(_) => {}
        Event::MessageCreate(msg) => {
            parse_command(&config, http, database, &msg).await?;
        }
        Event::Ready(_) => {}
        _ => println!("DEBUG: {event:?}"),
    }

    Ok(())
}

fn currently_electing(election: SystemTime, duration: Duration) -> Result<bool, SystemTimeError> {
    let current = SystemTime::now();
    Ok(current >= election && election.elapsed()? < duration)
}

fn get_next_election(epoch: SystemTime, interval: Duration) -> Result<SystemTime, Box<dyn Error>> {
    let seconds_since_epoch = SystemTime::now().duration_since(epoch)?;
    let intervals_since_epoch = seconds_since_epoch.as_secs() / interval.as_secs();
    let next_interval = (intervals_since_epoch + 1) * interval.as_secs();
    Ok(epoch.checked_add(Duration::from_secs(next_interval)).unwrap())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = toml::from_str::<KhaosControl>(&fs::read_to_string("khaos.toml")?)?;

    let mut shard = Shard::new(
        ShardId::ONE,
        config.token().clone(),
        Intents::GUILDS
            | Intents::GUILD_MEMBERS
            | Intents::GUILD_MESSAGES
            | Intents::MESSAGE_CONTENT,
    );

    let http = Arc::new(Client::new(config.token()));

    let cache = DefaultInMemoryCache::builder().build();

    let pool = Config::from_url(config.redis()).create_pool(Some(Runtime::Tokio1))?;

    // NOTE: Currently mut because next_election should change after an election ends ~ahill
    let mut next_election = get_next_election(config.epoch(), config.interval()).unwrap();

    while let Some(msg) = shard.next_event(EventTypeFlags::all()).await {
        let Ok(event) = msg else {
            eprintln!("Failed to receive event: {msg:?}");
            continue;
        };

        let database = pool.get().await?;

        cache.update(&event);

        if currently_electing(next_election, config.duration())? {
            // TODO: We are inside of an election
        }
        /*if !is_electing
            && current_time >= get_election_time(election_iter.await, config.epoch(), config.interval()).await
            && current_time <= config.duration()
        {
            send_message(Arc::clone(&http), "The biweekly election is now underway!", /*put channel_id here*/, None);
            is_electing = true;
        } else {
            is_electing = false;
        }*/
        //We need some way for handle_event to know when to accept election votes.
        tokio::spawn(handle_event(
            config.clone(),
            Arc::clone(&http),
            database,
            event,
        ));
    }

    Ok(())
}

async fn parse_command(
    config: &KhaosControl,
    http: Arc<Client>,
    database: Connection,
    msg: &Message,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    if msg.content.starts_with(config.prefix()) {
        let args: Vec<&str> = msg.content[config.prefix().len()..]
            .split_whitespace()
            .collect();

        match args[0] {
            "nominate" => {
                parse_nomination(
                    config,
                    http,
                    database,
                    msg.channel_id,
                    msg.author.id,
                    msg.id,
                    args[1],
                )
                .await?;
            }
            "vote" => {
                parse_vote(
                    config,
                    http,
                    database,
                    msg.channel_id,
                    msg.author.id,
                    msg.id,
                    args[1],
                )
                .await?;
            }
            _ => {}
        }
    }

    Ok(())
}

async fn parse_nomination(
    config: &KhaosControl,
    http: Arc<Client>,
    mut database: Connection,
    cid: Id<ChannelMarker>,
    author: Id<UserMarker>,
    mid: Id<MessageMarker>,
    nominee: &str,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let members = http
        .guild_members(config.guild())
        .limit(1000)
        .await?
        .models()
        .await?;
    let nominee = if nominee.starts_with("<@") && nominee.ends_with(">") {
        let id: Id<UserMarker> = nominee[2..nominee.len() - 1].parse()?;
        members.iter().find(|&member| member.user.id == id)
    } else {
        members.iter().find(|&member| member.user.name == nominee)
    };
    if let Some(nominee) = nominee {
        if redis::cmd("SADD")
            .arg(&[format!("nominee:{}", nominee.user.id), author.to_string()])
            .query_async(&mut database)
            .await?
        {
            println!("{author} nominated {}", nominee.user.id);
            http.create_reaction(cid, mid, &SUCCESS_REACTION).await?;
        } else {
            send_message(http, "You've already nominated this user!", cid, Some(mid)).await?;
        }
    }

    Ok(())
}

async fn parse_vote(
    config: &KhaosControl,
    http: Arc<Client>,
    mut database: Connection,
    cid: Id<ChannelMarker>,
    author: Id<UserMarker>,
    mid: Id<MessageMarker>,
    candidate: &str,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let members: Vec<String> = redis::cmd("KEYS")
        .arg("nominee:*")
        .query_async(&mut database)
        .await?;

    todo!()
}

async fn send_message(
    http: Arc<Client>,
    msg: &str,
    cid: Id<ChannelMarker>,
    rid: Option<Id<MessageMarker>>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    if let Some(value) = rid {
        http.create_message(cid).reply(value).content(&msg).await?;
    } else {
        http.create_message(cid).content(&msg).await?;
    }

    Ok(())
}
