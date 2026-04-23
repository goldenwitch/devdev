//! Chat message model.

/// Who sent the message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatRole {
    User,
    Agent,
    System,
}

/// A single chat message in the conversation history.
#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub text: String,
    pub complete: bool,
}

impl ChatMessage {
    pub fn user(text: impl Into<String>) -> Self {
        Self {
            role: ChatRole::User,
            text: text.into(),
            complete: true,
        }
    }

    pub fn agent(text: impl Into<String>, complete: bool) -> Self {
        Self {
            role: ChatRole::Agent,
            text: text.into(),
            complete,
        }
    }

    pub fn system(text: impl Into<String>) -> Self {
        Self {
            role: ChatRole::System,
            text: text.into(),
            complete: true,
        }
    }
}

/// Scrollable chat history.
pub struct ChatHistory {
    messages: Vec<ChatMessage>,
    scroll_offset: usize,
}

impl ChatHistory {
    pub fn new() -> Self {
        Self {
            messages: Vec::new(),
            scroll_offset: 0,
        }
    }

    pub fn push(&mut self, msg: ChatMessage) {
        self.messages.push(msg);
    }

    /// Append text to the last agent message, or create a new one.
    pub fn append_agent_text(&mut self, text: &str) {
        if let Some(last) = self.messages.last_mut()
            && last.role == ChatRole::Agent
            && !last.complete
        {
            last.text.push_str(text);
            return;
        }
        self.messages.push(ChatMessage::agent(text, false));
    }

    /// Mark the last agent message as complete.
    pub fn complete_agent_message(&mut self) {
        if let Some(last) = self.messages.last_mut()
            && last.role == ChatRole::Agent
        {
            last.complete = true;
        }
    }

    pub fn messages(&self) -> &[ChatMessage] {
        &self.messages
    }

    pub fn len(&self) -> usize {
        self.messages.len()
    }

    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    pub fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    pub fn scroll_up(&mut self, lines: usize) {
        self.scroll_offset = self.scroll_offset.saturating_add(lines);
        let max = self.messages.len().saturating_sub(1);
        if self.scroll_offset > max {
            self.scroll_offset = max;
        }
    }

    pub fn scroll_down(&mut self, lines: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(lines);
    }

    pub fn scroll_to_bottom(&mut self) {
        self.scroll_offset = 0;
    }
}

impl Default for ChatHistory {
    fn default() -> Self {
        Self::new()
    }
}
