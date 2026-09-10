//! Account-routed bot IO. The immutable target follows the submitted turn,
//! independently of later menu selection on the bot host.
use super::RemoteBotTarget;
use crate::remote_connect::{account::AccountClient, RemoteToolStatus};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};

impl RemoteBotTarget {
    async fn rpc_response(&self, command: Value) -> Result<Value, String> {
        let reply = AccountClient::new()
            .device_rpc(
                &self.relay_url,
                &self.account,
                &self.device_id,
                &command.to_string(),
            )
            .await
            .map_err(|e| e.to_string())?;
        serde_json::from_str(&reply).map_err(|e| e.to_string())
    }

    pub async fn poll(&self, version: u64) -> Result<Value, String> {
        self.rpc_response(json!({"cmd":"poll_session","session_id":self.session_id,"since_version":version,"known_msg_count":0})).await
    }

    pub async fn rpc(&self, command: Value) -> Result<Value, String> {
        let reply = self.rpc_response(command).await?;
        if reply["resp"] == "error" {
            return Err(reply["message"]
                .as_str()
                .or(reply["error"].as_str())
                .unwrap_or("Remote command failed")
                .to_string());
        }
        Ok(reply)
    }

    pub async fn read_file(
        &self,
        path: &str,
        max_bytes: u64,
        is_current: &(dyn Fn() -> bool + Sync),
    ) -> Result<super::WorkspaceFileContent, String> {
        // One host-side read returns a consistent byte snapshot, including on
        // older peers. Do not reopen the path on the controller.
        if !is_current() {
            return Err("Bot identity changed during output delivery".into());
        }
        let info = self
            .rpc(json!({"cmd":"get_file_info","path":path,"session_id":self.session_id}))
            .await?;
        let size = info["size"].as_u64().ok_or("Invalid remote file size")?;
        if info["resp"] != "file_info" || size > max_bytes {
            return Err(format!(
                "Remote file exceeds attachment limit ({max_bytes} bytes)"
            ));
        }
        if !is_current() {
            return Err("Bot identity changed during output delivery".into());
        }
        let file = self
            .rpc(json!({"cmd":"read_file","path":path,"session_id":self.session_id}))
            .await?;
        if !is_current() {
            return Err("Bot identity changed during output delivery".into());
        }
        decode_file(file, max_bytes)
    }
}

fn decode_file(file: Value, max_bytes: u64) -> Result<super::WorkspaceFileContent, String> {
    if file["resp"] != "file_content" {
        return Err("Invalid remote file response".into());
    }
    let size = file["size"].as_u64().ok_or("Invalid remote file size")?;
    if size > max_bytes {
        return Err(format!(
            "Remote file exceeds attachment limit ({max_bytes} bytes)"
        ));
    }
    let encoded = file["content_base64"]
        .as_str()
        .ok_or("Missing remote file bytes")?;
    if encoded.len() as u64 > max_bytes.div_ceil(3) * 4 {
        return Err("Remote file payload exceeds limit".into());
    }
    let bytes = STANDARD.decode(encoded).map_err(|e| e.to_string())?;
    if bytes.len() as u64 != size {
        return Err("Remote file byte count mismatch".into());
    }
    Ok(super::WorkspaceFileContent {
        name: file["name"]
            .as_str()
            .ok_or("Missing remote file name")?
            .to_string(),
        mime_type: super::detect_mime_type(std::path::Path::new(
            file["name"].as_str().ok_or("Missing remote file name")?,
        )),
        bytes,
        size,
    })
}

#[derive(Default, Debug)]
pub struct ObservedTurn {
    pub text: String,
    pub status: String,
    pub error: Option<String>,
    pub tools: Vec<RemoteToolStatus>,
}

impl ObservedTurn {
    pub fn terminal(&self) -> bool {
        matches!(
            self.status.as_str(),
            "done" | "completed" | "failed" | "cancelled" | "error"
        )
    }
}

/// Replay-safe projection, accepting the historical assistant ID when an older
/// peer omits turn_id. Never infer ownership from whichever turn is active now.
pub fn observe_turn(poll: &Value, turn_id: &str) -> Option<ObservedTurn> {
    let active = &poll["active_turn"];
    let message = poll["message_snapshot"]
        .as_array()
        .into_iter()
        .flatten()
        .chain(poll["new_messages"].as_array().into_iter().flatten())
        .find(|m| {
            m["role"] == "assistant"
                && (m["turn_id"] == turn_id
                    || (m["turn_id"].is_null() && m["id"] == format!("{turn_id}_assistant")))
        });
    let source = if active["turn_id"] == turn_id {
        active
    } else {
        message?
    };
    Some(ObservedTurn {
        text: source["text"]
            .as_str()
            .or(source["content"].as_str())
            .unwrap_or_default()
            .into(),
        status: source["status"].as_str().unwrap_or_default().into(),
        error: source["error"].as_str().map(str::to_string),
        tools: serde_json::from_value(source["tools"].clone()).unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn replay_binds_to_our_turn_even_after_next_turn_started() {
        let poll = json!({"active_turn":{"turn_id":"next","text":"other","status":"running"},
            "message_snapshot":[{"id":"ours_assistant","role":"assistant","content":"[report](report.pdf)","status":"done"}]});
        let result = observe_turn(&poll, "ours").unwrap();
        assert!(result.terminal());
        assert_eq!(result.text, "[report](report.pdf)");
        assert!(observe_turn(&poll, "unrelated").is_none());
        let conflict = json!({"new_messages":[{"id":"ours_assistant","turn_id":"other","role":"assistant","status":"done"}]});
        assert!(observe_turn(&conflict, "ours").is_none());
    }
    #[test]
    fn binary_reply_checks_size_and_preserves_bytes() {
        let file = json!({"resp":"file_content","name":"图.png","mime_type":"image/png","size":3,"content_base64":"AP8B"});
        assert_eq!(decode_file(file.clone(), 3).unwrap().bytes, vec![0, 255, 1]);
        assert!(decode_file(file.clone(), 2).is_err());
        let mut invalid = file;
        invalid["size"] = json!(2);
        assert!(decode_file(invalid, 3).is_err());
    }
}
