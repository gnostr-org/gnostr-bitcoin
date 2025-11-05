use anyhow::Result;
use arti_client::{IsolationToken, StreamPrefs, TorClient, TorClientConfig};
use bitcoin::{Transaction, p2p::message::NetworkMessage, p2p::ServiceFlags, p2p::message_blockdata::Inventory};

use clap::{Parser, arg, command};
use tor_rtcompat::PreferredRuntime;

use std::{collections::HashSet, sync::Arc};
use tokio::{net::lookup_host, sync::Semaphore, task::JoinSet, time::timeout, io::{self, AsyncReadExt, AsyncWriteExt}};

use tracing::{error, info};

use gnostr_bitcoin::{p2p::{self, NetworkAddress}, tor};

const MAX_CONCURRENT_DELIVERIES: usize = 100;
