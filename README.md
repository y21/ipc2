# `ipc2`
Easy inter process communication for Rust.

## Overview
This crate is made up of 3 smaller crates:
- `ipc2_host`
- `ipc2_worker`
- `ipc2_common`

The `ipc2_host` crate contains types and functions that the host process is interested in: spawning & managing worker processes, restarting worker processes, sending arbitrary  messages to worker processes.
<details>
    <summary>Example code</summary>

```rs
use ipc2_host::workerset::WorkerSet;
use tokio::net::UnixListener;
use test_common::HostMessage;
use std::error::Error;

type MyWorkerSet = WorkerSet<
    /* Type of server to use for IPC */ UnixListener,
    /* Type of worker->host message */ u32,
    /* Type of host->worker message */ HostMessage
>;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let workers: MyWorkerSet = WorkerSet::builder()
        .worker_count(4)
        .worker_path("./target/debug/worker_test")
        .finish()
        .await?;

    // Let the worker process add two numbers together
    // And receive a `u32` in response.
    let resp: u32 = workers.send(HostMessage::Add(6, 7)).await?;

    println!("Result -> {}", resp);

    Ok(())
}

```
It starts by building a `WorkerSet`, which owns a set of worker processes and manages those. You can configure how many workers it should spawn, or leave it at the default value, which is however many cores there are available. Finally, a path must be provided to the worker binary.

With the `WorkerSet` you can then send jobs to the worker process. Calling `send` will try to pick an available worker that is currently not processing a request.
</details>

<hr />

The `ipc2_worker` crate contains types and functions for worker processes. It connects to the host process and lets you receive messages from the parent process, as well as responding to them.

<details>
    <summary>Example code</summary>

```rs
use ipc2_worker::Job;
use test_common::HostMessage;
use test_common::WorkerMessage;
use tokio::net::UnixStream;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut rx = ipc2_worker::connect::<
        /* Type of client to use for IPC */ UnixStream,
        /* Type of host->worker message */ HostMessage,
        /* Type of worker->host message */ u32
    >().await?;

    while let Some(job) = rx.recv().await {
        match job {
            Job::Bidirectional { data, tx } => {
                let _ = tx.send(match data {
                    HostMessage::Add(a, b) => a + b,
                    HostMessage::Sub(a, b) => a - b
                });
            },
            Job::Unidirectional { .. } => unreachable!()
        };
    }

    Ok(())
}

```
The worker process should call `connect()` and start handling incoming messages from the returned receiver.

It is generic over the client (`C: Client`), the message being received (`R: Deserialize`) and the message beint sent back (`W: Serialize`).

Calling `recv()` on the channel will pause the task until a message is received from the host. The data is available through the `data` field and you can respond by calling `send()` on the `tx` in the job.
</details>

NOTE: Apart from a host and a worker crate, you'll also likely need a third crate if you want to send custom types that both the worker and host crate can refer to.
<details>
    <summary>Example code for a third crate</summary>

```rs
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
pub enum HostMessage {
    Add(u32, u32),
    Sub(u32, u32),
}
```
</details>

<hr />

The `ipc2_common` is currently only used by `ipc2` itself, for functionality that both `_worker` and `_host` need. As a user of this crate you likely won't need this.

## Communication strategy
Most host types are generic over the server, so you can decide how the host sends its messages to the client by implementing the `Server` trait for your type. If you don't care how communication happens and the default strategy is good enough, you can simply pass `UnixListener` as `S`.
Similarily, the worker is generic over the client, and `Client` is implemented for `UnixStream`.
