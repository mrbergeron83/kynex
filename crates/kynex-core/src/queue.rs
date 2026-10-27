//! Messages written while Claude works, waiting for the turn to end.
//!
//! Each session keeps its own queue. Like Claude Code, whenever a turn ends,
//! finished or interrupted (Esc), everything queued goes out together as the
//! next message, in the order it was written; "Send now" (Ctrl+Enter) just
//! interrupts the turn to get there sooner.

use crate::session::Attachment;

/// A message waiting to be sent.
pub struct Queued {
    pub text: String,
    pub attachments: Vec<Attachment>,
}

#[derive(Default)]
pub struct Queue {
    items: Vec<Queued>,
}

impl Queue {
    pub fn items(&self) -> &[Queued] {
        &self.items
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn push(&mut self, text: String, attachments: Vec<Attachment>) {
        self.items.push(Queued { text, attachments });
    }

    /// Takes a message out, to drop it or to edit it in the composer.
    pub fn remove(&mut self, i: usize) -> Option<Queued> {
        (i < self.items.len()).then(|| self.items.remove(i))
    }

    /// Empties the queue into one message: the texts in order, a blank line
    /// apart, and all the attachments. `None` if nothing is queued.
    pub fn take_all(&mut self) -> Option<Queued> {
        if self.items.is_empty() {
            return None;
        }
        let mut texts = Vec::new();
        let mut attachments = Vec::new();
        for m in self.items.drain(..) {
            if !m.text.trim().is_empty() {
                texts.push(m.text);
            }
            attachments.extend(m.attachments);
        }
        Some(Queued { text: texts.join("\n\n"), attachments })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn queue(texts: &[&str]) -> Queue {
        let mut q = Queue::default();
        for t in texts {
            q.push(t.to_string(), Vec::new());
        }
        q
    }

    #[test]
    fn takes_everything_as_one_message_in_order() {
        let mut q = queue(&["first", "", "second"]);
        assert_eq!(q.take_all().map(|m| m.text), Some("first\n\nsecond".into()));
        assert!(q.is_empty());
        assert!(q.take_all().is_none());
    }

    #[test]
    fn removes_by_index() {
        let mut q = queue(&["a", "b"]);
        assert_eq!(q.remove(0).map(|m| m.text), Some("a".into()));
        assert!(q.remove(3).is_none());
        assert_eq!(q.items().iter().map(|m| m.text.as_str()).collect::<Vec<_>>(), ["b"]);
    }
}
