use serde::Deserialize;
use serde::Serialize;

#[derive(Deserialize, Serialize)]
pub enum ServerSocketMessage<T> {
    Unidirectional { data: T },
    Bidirectional { data: T, job_id: u32 },
}

#[derive(Deserialize, Serialize)]
pub enum ClientSocketMessage<T> {
    Response { data: T, job_id: u32 },
}
