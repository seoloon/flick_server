//! Lightweight in-memory room chat.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

use crate::errors::{Error, ErrorCode, Result};
use crate::sync::clock::Millis;

#[derive(Debug, Clone)]
pub struct ChatConfig {
    pub max_message_chars: usize,
    pub max_history: usize,
    /// Sustained messages per second per participant.
    pub rate_per_sec: f64,
    pub burst: f64,
}

impl Default for ChatConfig {
    fn default() -> Self {
        Self {
            max_message_chars: 500,
            max_history: 100,
            rate_per_sec: 1.0,
            burst: 5.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatMessage {
    /// Unique and increasing within the room.
    pub id: u64,
    pub room_id: String,
    pub sender_id: String,
    pub sender_name: String,
    pub text: String,
    /// Server wall clock, ms since Unix epoch.
    pub timestamp: Millis,
}

/// Remove control characters (newlines and tabs become spaces), collapse
/// surrounding whitespace and cap the length in characters.
///
/// Output is plain text: clients must render it as text, never as markup.
pub fn sanitize_text(input: &str, max_chars: usize) -> String {
    let cleaned: String = input
        .chars()
        .filter_map(|c| match c {
            '\n' | '\t' | '\r' => Some(' '),
            c if c.is_control() => None,
            // Zero-width / bidi override characters are used for spoofing.
            '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' => None,
            c => Some(c),
        })
        .collect();
    cleaned.trim().chars().take(max_chars).collect()
}

/// Bounded chat history for one room.
#[derive(Debug)]
pub struct ChatLog {
    messages: VecDeque<ChatMessage>,
    next_id: u64,
}

impl Default for ChatLog {
    fn default() -> Self {
        Self::new()
    }
}

impl ChatLog {
    pub fn new() -> Self {
        Self {
            messages: VecDeque::new(),
            next_id: 1,
        }
    }

    pub fn push(
        &mut self,
        cfg: &ChatConfig,
        room_id: &str,
        sender_id: &str,
        sender_name: &str,
        raw_text: &str,
        timestamp: Millis,
    ) -> Result<ChatMessage> {
        if raw_text.chars().count() > cfg.max_message_chars * 4 {
            return Err(Error::new(
                ErrorCode::MessageTooLarge,
                "chat message too long",
            ));
        }
        let text = sanitize_text(raw_text, usize::MAX);
        if text.is_empty() {
            return Err(Error::new(
                ErrorCode::InvalidPayload,
                "chat message is empty",
            ));
        }
        if text.chars().count() > cfg.max_message_chars {
            return Err(Error::new(
                ErrorCode::MessageTooLarge,
                format!("chat message exceeds {} characters", cfg.max_message_chars),
            ));
        }
        let msg = ChatMessage {
            id: self.next_id,
            room_id: room_id.to_owned(),
            sender_id: sender_id.to_owned(),
            sender_name: sender_name.to_owned(),
            text,
            timestamp,
        };
        self.next_id += 1;
        self.messages.push_back(msg.clone());
        while self.messages.len() > cfg.max_history {
            self.messages.pop_front();
        }
        Ok(msg)
    }

    pub fn history(&self) -> Vec<ChatMessage> {
        self.messages.iter().cloned().collect()
    }

    pub fn len(&self) -> usize {
        self.messages.len()
    }

    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_strips_controls_and_bidi() {
        assert_eq!(
            sanitize_text("  hi\u{0000}\nthere\u{202E} ", 100),
            "hi there"
        );
        assert_eq!(
            sanitize_text("<b>x</b>", 100),
            "<b>x</b>",
            "no markup rewriting"
        );
    }

    #[test]
    fn sanitize_truncates_by_chars_not_bytes() {
        assert_eq!(sanitize_text("ééééé", 3), "ééé");
    }

    #[test]
    fn push_rejects_empty_and_oversized() {
        let cfg = ChatConfig::default();
        let mut log = ChatLog::new();
        assert_eq!(
            log.push(&cfg, "r", "u", "U", " \n ", 0).unwrap_err().code,
            ErrorCode::InvalidPayload
        );
        let huge = "a".repeat(cfg.max_message_chars * 4 + 1);
        assert_eq!(
            log.push(&cfg, "r", "u", "U", &huge, 0).unwrap_err().code,
            ErrorCode::MessageTooLarge
        );
        assert!(log.is_empty());
    }

    #[test]
    fn history_is_bounded_and_ids_increase() {
        let cfg = ChatConfig {
            max_history: 3,
            ..ChatConfig::default()
        };
        let mut log = ChatLog::new();
        for i in 0..5 {
            log.push(&cfg, "r", "u", "U", &format!("m{i}"), i).unwrap();
        }
        let h = log.history();
        assert_eq!(h.len(), 3);
        assert_eq!(h[0].text, "m2");
        assert_eq!(h[2].id, 5);
    }

    #[test]
    fn overlong_message_is_rejected() {
        let cfg = ChatConfig {
            max_message_chars: 10,
            ..ChatConfig::default()
        };
        let mut log = ChatLog::new();
        let err = log
            .push(&cfg, "r", "u", "U", &"x".repeat(30), 0)
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::MessageTooLarge);
        assert!(log.push(&cfg, "r", "u", "U", &"x".repeat(10), 0).is_ok());
    }
}
