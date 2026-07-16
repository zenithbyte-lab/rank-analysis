//! # LCU 聊天 API
//!
//! 封装选人阶段队伍聊天：获取当前会话 ID、发送消息。
//! 用于自动发送「马评级」等到英雄选择聊天室。

use crate::lcu::util::http::{lcu_get, lcu_post};
use serde::{Deserialize, Serialize};

/// 选人阶段会话类型（LCU `/lol-chat/v1/conversations` 的 `type` 字段）
const CONVERSATION_TYPE_CHAMP_SELECT: &str = "championSelect";

/// 聊天会话摘要（只取发送消息需要的字段）
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Conversation {
    pub id: String,
    #[serde(rename = "type")]
    pub conversation_type: String,
}

/// 发送消息请求体
#[derive(Debug, Serialize)]
struct SendMessageBody<'a> {
    body: &'a str,
    #[serde(rename = "type")]
    msg_type: &'a str,
}

/// 获取当前英雄选择阶段的聊天会话 ID。
///
/// # 返回值
///
/// - `Ok(id)`: 当前处于选人阶段时的会话 ID
/// - `Err`: 不在选人阶段，或 LCU 请求失败
pub async fn get_champ_select_conversation_id() -> Result<String, String> {
    let list: Vec<Conversation> = lcu_get("/lol-chat/v1/conversations").await?;
    list.into_iter()
        .find(|c| c.conversation_type == CONVERSATION_TYPE_CHAMP_SELECT)
        .map(|c| c.id)
        .ok_or_else(|| "当前不在英雄选择阶段".to_string())
}

/// 向指定聊天会话发送一条消息。
///
/// # 参数
///
/// - `conversation_id`: 会话 ID（通常来自 `get_champ_select_conversation_id`）
/// - `msg`: 消息正文
///
/// # 注意
///
/// LCU 对发送频率有限制，连续发送建议间隔 ≥ 2 秒。
pub async fn send_conversation_msg(conversation_id: &str, msg: &str) -> Result<(), String> {
    let uri = format!("/lol-chat/v1/conversations/{}/messages", conversation_id);
    let body = SendMessageBody {
        body: msg,
        msg_type: "chat",
    };
    // 成功时常返回消息对象或 204；反序列化为 serde_json::Value 兼容空 body / 对象
    let _: serde_json::Value = lcu_post(&uri, &body).await?;
    Ok(())
}
