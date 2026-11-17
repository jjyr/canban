//! Presentation-only Board filters, shared by desktop persistence and UI.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct BoardFilter {
    /// Empty, local, github, issue or pr. Source and type form one choice.
    pub kind: String,
    /// Empty = anyone; mine = current account; author = a specific login.
    pub author_mode: String,
    pub author: String,
    pub state: String,
    pub scheduled: bool,
    pub tags: Vec<String>,
}
impl BoardFilter {
    pub fn active(&self) -> bool {
        self != &Self::default()
    }
    pub fn valid(&self) -> bool {
        matches!(self.kind.as_str(), "" | "local" | "github" | "issue" | "pr")
            && matches!(self.author_mode.as_str(), "" | "mine" | "author")
            && (self.author_mode == "author") == !self.author.is_empty()
            && self.author.len() <= 256
            && !self.author.chars().any(char::is_control)
            && matches!(
                self.state.as_str(),
                "" | "open" | "closed" | "draft" | "merged"
            )
            && (self.state.is_empty() || matches!(self.kind.as_str(), "github" | "issue" | "pr"))
            && self.tags.len() <= 100
            && self
                .tags
                .iter()
                .all(|t| crate::normalize_tag(t).as_ref() == Some(t))
    }
    pub fn set_kind(&mut self, kind: &str) {
        self.kind = kind.into();
        if !matches!(kind, "github" | "issue" | "pr") {
            self.state.clear();
        }
    }
    pub fn set_state(&mut self, state: &str) {
        self.state = state.into();
        if !state.is_empty() && !matches!(self.kind.as_str(), "github" | "issue" | "pr") {
            self.kind = "github".into();
        }
    }
    pub fn set_author(&mut self, mode: &str, author: &str) {
        self.author_mode = mode.into();
        self.author = if mode == "author" {
            author.into()
        } else {
            String::new()
        };
    }
    /// Unknown source authors must never fall back to the importing user.
    pub fn matches_card(
        &self,
        card: &crate::Card,
        cached: Option<&crate::GitHubIssueSnapshot>,
        login: Option<&str>,
    ) -> bool {
        let source = if card.github_issue().is_none() {
            "local"
        } else {
            card.source_kind()
        };
        if !(self.kind.is_empty()
            || self.kind == source
            || self.kind == "github" && source != "local")
            || self.scheduled && card.schedule_source.is_none()
        {
            return false;
        }
        let author = if source == "local" {
            Some(card.created_by.as_str())
        } else {
            cached
                .map(|v| v.author_login.as_str())
                .filter(|a| !a.is_empty())
        };
        if self.author_mode == "mine" && login.is_none() && source != "local" {
            return false;
        }
        let wanted = match self.author_mode.as_str() {
            "mine" => Some(login.unwrap_or("Local")),
            "author" => Some(self.author.as_str()),
            _ => None,
        };
        if let Some(wanted) = wanted
            && !author.is_some_and(|a| a.eq_ignore_ascii_case(wanted))
        {
            return false;
        }
        self.state.is_empty() || cached.is_some_and(|v| v.state == self.state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Card, CardContent, GitHubIssueRef, GitHubIssueSnapshot};
    use serde_json::json;
    fn card() -> Card {
        serde_json::from_value(json!({"id":"c","project_id":"p","number":1,"title":"Test","body":"","stage":"backlog","revision":1,"created_at":0,"created_by":"Alice"})).unwrap()
    }
    fn source() -> GitHubIssueRef {
        GitHubIssueRef {
            repository: "example/repo".into(),
            repository_id: 1,
            issue_id: 2,
            number: 2,
            url: "https://github.com/example/repo/pull/2".into(),
        }
    }
    fn cached() -> GitHubIssueSnapshot {
        GitHubIssueSnapshot {
            id: "remote".into(),
            project_id: "p".into(),
            source: source(),
            title: "PR".into(),
            body: String::new(),
            state: "open".into(),
            author_id: 1,
            author_login: "ALICE".into(),
            labels: vec![],
            assignees: vec![],
            updated_at: String::new(),
            synced_at: 0,
        }
    }
    #[test]
    fn mine_covers_local_issue_pr_and_never_the_importer_or_assignee() {
        let mut f = BoardFilter::default();
        f.set_author("mine", "");
        let mut c = card();
        let mut cache = cached();
        assert!(f.matches_card(&c, None, Some("alice")));
        assert!(!f.matches_card(&c, None, Some("bob")));
        for content in [
            CardContent::GitHubIssue { source: source() },
            CardContent::GitHubPr { source: source() },
        ] {
            c.content = content;
            assert!(f.matches_card(&c, Some(&cache), Some("alice")));
            assert!(!f.matches_card(&c, None, Some("alice")));
            cache.author_login = "Bob".into();
            cache.assignees = vec!["alice".into()];
            assert!(!f.matches_card(&c, Some(&cache), Some("alice")));
            cache.author_login = "ALICE".into();
        }
        assert!(!f.matches_card(&c, Some(&cache), None));
        c.content = CardContent::Local;
        c.created_by = "Local".into();
        assert!(f.matches_card(&c, None, None));
        assert!(!f.matches_card(&c, None, Some("alice")));
    }
    #[test]
    fn changing_author_replaces_mine_and_type_state_never_contradict() {
        let mut f = BoardFilter::default();
        f.set_author("mine", "");
        f.set_author("author", "bob");
        assert!(!f.matches_card(&card(), None, Some("alice")));
        f.set_state("open");
        assert_eq!(f.kind, "github");
        assert!(f.valid());
        f.set_kind("local");
        assert!(f.state.is_empty());
        assert!(f.valid());
        f.set_author("", "ignored");
        assert!(f.author.is_empty());
        f.scheduled = true;
        assert!(!f.matches_card(&card(), None, Some("alice")));
        f = BoardFilter::default();
        f.set_kind("pr");
        assert!(!f.matches_card(&card(), None, Some("alice")));
        f = BoardFilter::default();
        assert!(!f.active());
        assert!(f.valid());
    }
}
