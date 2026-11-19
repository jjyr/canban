//! One visible command catalog for global bindings, Terminal keys and Command Palette (`⌘/`, `/`, `>…`).
//!
//! Every user-executable UI operation across global chrome, Activity & Notifications,
//! Board, Card Workspace, Git, Terminals, Managed Worktrees, GitHub, and
//! Settings is registered here and dispatches the same underlying intent as UI controls.
use crate::presenter::*;
use nsh_core::{keymap::When, prelude::*};
use serde_json::json;

#[derive(Clone, Copy)]
pub struct Command {
    pub id: &'static str,
    pub title: &'static str,
    pub key: &'static str,
    pub display: &'static str,
    pub context: &'static str,
    pub keywords: &'static str,
}

macro_rules! platform_str {
    ($mac:expr, $other:expr) => {{
        #[cfg(target_os = "macos")]
        {
            $mac
        }
        #[cfg(not(target_os = "macos"))]
        {
            $other
        }
    }};
}

pub mod shortcuts {
    pub const SUBMIT_KEY: &str = platform_str!("mod+enter", "ctrl+enter");
    pub const SUBMIT_DISPLAY: &str = platform_str!("⌘Enter", "Ctrl+Enter");

    pub const SAVE_WORKFLOW_KEY: &str = platform_str!("mod+s", "ctrl+s");
    pub const SAVE_WORKFLOW_DISPLAY: &str = platform_str!("⌘S", "Ctrl+S");

    pub const NOTIFICATIONS_KEY: &str = platform_str!("mod+i", "ctrl+shift+i");
    pub const NOTIFICATIONS_DISPLAY: &str = platform_str!("⌘I", "Ctrl+Shift+I");

    pub const NEW_CARD_KEY: &str = platform_str!("mod+n", "ctrl+shift+n");
    pub const NEW_CARD_DISPLAY: &str = platform_str!("⌘N", "Ctrl+Shift+N");

    pub const QUICK_OPEN_KEY: &str = platform_str!("mod+o", "ctrl+shift+o");
    pub const QUICK_OPEN_DISPLAY: &str = platform_str!("⌘O", "Ctrl+Shift+O");

    pub const COMMANDS_KEY: &str = platform_str!("mod+/", "ctrl+shift+/");
    pub const COMMANDS_DISPLAY: &str = platform_str!("⌘/", "Ctrl+Shift+/");

    pub const CARD_TERMINAL_KEY: &str = platform_str!("mod+j", "ctrl+shift+j");
    pub const CARD_TERMINAL_DISPLAY: &str = platform_str!("⌘J", "Ctrl+Shift+J");

    pub const NEW_TERMINAL_KEY: &str = platform_str!("mod+t", "ctrl+shift+t");
    pub const NEW_TERMINAL_DISPLAY: &str = platform_str!("⌘T", "Ctrl+Shift+T");

    pub const CLOSE_TERMINAL_KEY: &str = platform_str!("mod+w", "ctrl+shift+w");
    pub const CLOSE_TERMINAL_DISPLAY: &str = platform_str!("⌘W", "Ctrl+Shift+W");

    pub const NEXT_TAB_KEY: &str = platform_str!("mod+shift+]", "ctrl+shift+]");
    pub const NEXT_TAB_DISPLAY: &str = platform_str!("⌘⇧]", "Ctrl+Shift+]");

    pub const PREV_TAB_KEY: &str = platform_str!("mod+shift+[", "ctrl+shift+[");
    pub const PREV_TAB_DISPLAY: &str = platform_str!("⌘⇧[", "Ctrl+Shift+[");

    pub const BACK_KEY: &str = platform_str!("mod+[", "ctrl+alt+[");
    pub const BACK_DISPLAY: &str = platform_str!("⌘[", "Ctrl+Alt+[");

    pub const FORWARD_KEY: &str = platform_str!("mod+]", "ctrl+alt+]");
    pub const FORWARD_DISPLAY: &str = platform_str!("⌘]", "Ctrl+Alt+]");
}

pub const CREATE_CARD: Command = Command {
    id: "create-card",
    title: "Create Card",
    key: shortcuts::SUBMIT_KEY,
    display: shortcuts::SUBMIT_DISPLAY,
    context: "New Card",
    keywords: "submit",
};

pub const SEND_COMMENT: Command = Command {
    id: "send-comment",
    title: "Send comment",
    key: shortcuts::SUBMIT_KEY,
    display: shortcuts::SUBMIT_DISPLAY,
    context: "Card comment",
    keywords: "reply",
};

pub const CATALOG: &[Command] = &[
    Command {
        id: "toggle-notifications",
        title: "Toggle Activity & Notifications",
        key: shortcuts::NOTIFICATIONS_KEY,
        display: shortcuts::NOTIFICATIONS_DISPLAY,
        context: "Global",
        keywords: "overview alerts unread inbox running status slots",
    },
    Command {
        id: "toggle-hosts",
        title: "Toggle Connected Hosts",
        key: "",
        display: "Hosts",
        context: "Global",
        keywords: "hosts machines tailscale ssh remote server canband",
    },
    Command {
        id: "connect-machine",
        title: "Connect a Machine…",
        key: "",
        display: "Hosts",
        context: "Global",
        keywords: "connect pair machine host tailscale ssh linux remote canband",
    },
    Command {
        id: "go-diagnostics",
        title: "Open Diagnostics",
        key: "",
        display: "Settings",
        context: "Preferences",
        keywords: "diagnostics health logs server.log herdr sqlite latency tailscale",
    },
    Command {
        id: "go-hosts",
        title: "Manage Connected Hosts",
        key: "",
        display: "Settings",
        context: "Preferences",
        keywords: "hosts remote servers tailscale ssh remove rename",
    },
    Command {
        id: "reconnect-hosts",
        title: "Reconnect Remote Hosts",
        key: "",
        display: "Hosts",
        context: "Global",
        keywords: "reconnect retry offline sync hosts remote",
    },
    Command {
        id: "new-card",
        title: "New Card",
        key: shortcuts::NEW_CARD_KEY,
        display: shortcuts::NEW_CARD_DISPLAY,
        context: "Global",
        keywords: "task create",
    },
    Command {
        id: "quick-open",
        title: "Quick Open",
        key: shortcuts::QUICK_OPEN_KEY,
        display: shortcuts::QUICK_OPEN_DISPLAY,
        context: "Global",
        keywords: "search jump",
    },
    Command {
        id: "shortcuts",
        title: "Commands & Shortcuts",
        key: shortcuts::COMMANDS_KEY,
        display: shortcuts::COMMANDS_DISPLAY,
        context: "Global",
        keywords: "commands palette >",
    },
    Command {
        id: "toggle-card-terminal",
        title: "Toggle Card Live Terminal",
        key: shortcuts::CARD_TERMINAL_KEY,
        display: shortcuts::CARD_TERMINAL_DISPLAY,
        context: "Card",
        keywords: "split cli side-by-side discuss conversation",
    },
    Command {
        id: "new-terminal",
        title: "New Terminal",
        key: shortcuts::NEW_TERMINAL_KEY,
        display: shortcuts::NEW_TERMINAL_DISPLAY,
        context: "Global",
        keywords: "shell pty standalone",
    },
    Command {
        id: "new-worktree",
        title: "New Managed Branch Worktree…",
        key: "",
        display: "Terminals",
        context: "Worktrees",
        keywords: "git branch checkout worktree spike standalone",
    },
    Command {
        id: "close-terminal",
        title: "Close Terminal",
        key: shortcuts::CLOSE_TERMINAL_KEY,
        display: shortcuts::CLOSE_TERMINAL_DISPLAY,
        context: "Terminals",
        keywords: "tab",
    },
    Command {
        id: "next-tab",
        title: "Next Terminal tab",
        key: shortcuts::NEXT_TAB_KEY,
        display: shortcuts::NEXT_TAB_DISPLAY,
        context: "Terminals",
        keywords: "",
    },
    Command {
        id: "previous-tab",
        title: "Previous Terminal tab",
        key: shortcuts::PREV_TAB_KEY,
        display: shortcuts::PREV_TAB_DISPLAY,
        context: "Terminals",
        keywords: "",
    },
    Command {
        id: "terminal-tab-1",
        title: "Switch to Terminal Tab 1",
        key: platform_str!("mod+1", "alt+1"),
        display: platform_str!("⌘1", "Alt+1"),
        context: "Terminals",
        keywords: "tab number",
    },
    Command {
        id: "terminal-tab-2",
        title: "Switch to Terminal Tab 2",
        key: platform_str!("mod+2", "alt+2"),
        display: platform_str!("⌘2", "Alt+2"),
        context: "Terminals",
        keywords: "tab number",
    },
    Command {
        id: "terminal-tab-3",
        title: "Switch to Terminal Tab 3",
        key: platform_str!("mod+3", "alt+3"),
        display: platform_str!("⌘3", "Alt+3"),
        context: "Terminals",
        keywords: "tab number",
    },
    Command {
        id: "terminal-tab-4",
        title: "Switch to Terminal Tab 4",
        key: platform_str!("mod+4", "alt+4"),
        display: platform_str!("⌘4", "Alt+4"),
        context: "Terminals",
        keywords: "tab number",
    },
    Command {
        id: "terminal-tab-5",
        title: "Switch to Terminal Tab 5",
        key: platform_str!("mod+5", "alt+5"),
        display: platform_str!("⌘5", "Alt+5"),
        context: "Terminals",
        keywords: "tab number",
    },
    Command {
        id: "terminal-tab-6",
        title: "Switch to Terminal Tab 6",
        key: platform_str!("mod+6", "alt+6"),
        display: platform_str!("⌘6", "Alt+6"),
        context: "Terminals",
        keywords: "tab number",
    },
    Command {
        id: "terminal-tab-7",
        title: "Switch to Terminal Tab 7",
        key: platform_str!("mod+7", "alt+7"),
        display: platform_str!("⌘7", "Alt+7"),
        context: "Terminals",
        keywords: "tab number",
    },
    Command {
        id: "terminal-tab-8",
        title: "Switch to Terminal Tab 8",
        key: platform_str!("mod+8", "alt+8"),
        display: platform_str!("⌘8", "Alt+8"),
        context: "Terminals",
        keywords: "tab number",
    },
    Command {
        id: "terminal-tab-9",
        title: "Switch to Terminal Tab 9",
        key: platform_str!("mod+9", "alt+9"),
        display: platform_str!("⌘9", "Alt+9"),
        context: "Terminals",
        keywords: "tab number",
    },
    CREATE_CARD,
    SEND_COMMENT,
    Command {
        id: "save-card",
        title: "Save Card changes",
        key: shortcuts::SUBMIT_KEY,
        display: shortcuts::SUBMIT_DISPLAY,
        context: "Card editor",
        keywords: "",
    },
    Command {
        id: "back",
        title: "Back",
        key: shortcuts::BACK_KEY,
        display: shortcuts::BACK_DISPLAY,
        context: "Global",
        keywords: "history",
    },
    Command {
        id: "forward",
        title: "Forward",
        key: shortcuts::FORWARD_KEY,
        display: shortcuts::FORWARD_DISPLAY,
        context: "Global",
        keywords: "history",
    },
    Command {
        id: "mark-all-read",
        title: "Mark All Updates as Read",
        key: "",
        display: "Notifications",
        context: "Notifications",
        keywords: "notifications clear unread overview",
    },
    Command {
        id: "go-board",
        title: "Go to Board",
        key: "",
        display: "Board",
        context: "Navigation",
        keywords: "kanban cards",
    },
    Command {
        id: "filter-board-mine",
        title: "Filter Board: Mine",
        key: "",
        display: "Board",
        context: "Board",
        keywords: "author me my cards created by me filter",
    },
    Command {
        id: "filter-board-more",
        title: "Filter Board: GitHub State or Scheduled…",
        key: "",
        display: "Board",
        context: "Board",
        keywords: "more filters open closed draft merged scheduled",
    },
    Command {
        id: "clear-board-filters",
        title: "Clear Board Filters",
        key: "",
        display: "Board",
        context: "Board",
        keywords: "tag filter reset show all",
    },
    Command {
        id: "go-terminals",
        title: "Go to Terminals",
        key: "",
        display: "Terminals",
        context: "Navigation",
        keywords: "shell cli",
    },
    Command {
        id: "go-review",
        title: "Go to Git",
        key: "",
        display: "Git",
        context: "Navigation",
        keywords: "diff review compare patch branch commit",
    },
    Command {
        id: "go-github",
        title: "Go to GitHub",
        key: "",
        display: "GitHub",
        context: "Navigation",
        keywords: "issues pull requests pr",
    },
    Command {
        id: "go-insights",
        title: "Go to Insights",
        key: "",
        display: "Insights",
        context: "Navigation",
        keywords: "statistics metrics charts kanban merged completion scheduling throughput cadence",
    },
    Command {
        id: "insights-today",
        title: "Insights: Show Today (1D)",
        key: "",
        display: "Insights",
        context: "Insights",
        keywords: "statistics daily date comparison trend agents samples efficiency",
    },
    Command {
        id: "insights-previous-day",
        title: "Insights: Previous Day",
        key: "",
        display: "Insights",
        context: "Insights",
        keywords: "statistics daily date comparison trend agents samples efficiency",
    },
    Command {
        id: "insights-next-day",
        title: "Insights: Next Day",
        key: "",
        display: "Insights",
        context: "Insights",
        keywords: "statistics daily date comparison trend agents samples efficiency",
    },
    Command {
        id: "insights-apply-date",
        title: "Insights: Apply Entered Date",
        key: "",
        display: "Insights",
        context: "Insights",
        keywords: "statistics daily date comparison trend agents samples efficiency",
    },
    Command {
        id: "insights-compare-previous",
        title: "Insights: Compare Previous Period",
        key: "",
        display: "Insights",
        context: "Insights",
        keywords: "statistics daily date comparison trend agents samples efficiency",
    },
    Command {
        id: "insights-compare-week",
        title: "Insights: Compare Last Week",
        key: "",
        display: "Insights",
        context: "Insights",
        keywords: "statistics daily date comparison trend agents samples efficiency",
    },
    Command {
        id: "insights-metric-merged",
        title: "Insights: Chart Resolved Cards",
        key: "",
        display: "Insights",
        context: "Insights",
        keywords: "statistics daily date comparison trend agents samples efficiency resolved merged",
    },
    Command {
        id: "insights-metric-created",
        title: "Insights: Chart Created Cards",
        key: "",
        display: "Insights",
        context: "Insights",
        keywords: "statistics daily date comparison trend agents samples efficiency",
    },
    Command {
        id: "insights-metric-runs",
        title: "Insights: Chart Agent Starts",
        key: "",
        display: "Insights",
        context: "Insights",
        keywords: "statistics daily date comparison trend agents samples efficiency",
    },
    Command {
        id: "insights-efficiency",
        title: "Insights: Toggle Efficiency Details",
        key: "",
        display: "Insights",
        context: "Insights",
        keywords: "statistics daily date comparison trend agents samples efficiency",
    },
    Command {
        id: "insights-samples-cycle",
        title: "Insights: Inspect Merge Timing Samples",
        key: "",
        display: "Insights",
        context: "Insights",
        keywords: "statistics daily date comparison trend agents samples efficiency",
    },
    Command {
        id: "insights-samples-queue",
        title: "Insights: Inspect Queue Timing Samples",
        key: "",
        display: "Insights",
        context: "Insights",
        keywords: "statistics daily date comparison trend agents samples efficiency",
    },
    Command {
        id: "insights-samples-next",
        title: "Insights: Next Sample Page",
        key: "",
        display: "Insights",
        context: "Insights",
        keywords: "statistics daily date comparison trend agents samples efficiency",
    },
    Command {
        id: "insights-samples-previous",
        title: "Insights: Previous Sample Page",
        key: "",
        display: "Insights",
        context: "Insights",
        keywords: "statistics daily date comparison trend agents samples efficiency",
    },
    Command {
        id: "insights-group-provider",
        title: "Insights: Group Agents by Provider",
        key: "",
        display: "Insights",
        context: "Insights",
        keywords: "statistics daily date comparison trend agents samples efficiency",
    },
    Command {
        id: "insights-group-profile",
        title: "Insights: Group Agents by Profile",
        key: "",
        display: "Insights",
        context: "Insights",
        keywords: "statistics daily date comparison trend agents samples efficiency",
    },
    Command {
        id: "insights-sort-created",
        title: "Insights: Sort Projects by Created",
        key: "",
        display: "Insights",
        context: "Insights",
        keywords: "statistics daily date comparison trend agents samples efficiency",
    },
    Command {
        id: "insights-sort-merged",
        title: "Insights: Sort Projects by Resolved",
        key: "",
        display: "Insights",
        context: "Insights",
        keywords: "statistics daily date comparison trend agents samples efficiency resolved merged",
    },
    Command {
        id: "insights-cards-created",
        title: "Insights: Inspect Created Cards",
        key: "",
        display: "Insights",
        context: "Insights",
        keywords: "statistics daily date comparison trend agents samples efficiency",
    },
    Command {
        id: "insights-cards-merged",
        title: "Insights: Inspect Resolved Cards",
        key: "",
        display: "Insights",
        context: "Insights",
        keywords: "statistics daily date comparison trend agents samples efficiency resolved merged",
    },
    Command {
        id: "insights-cards-close",
        title: "Insights: Close Card List",
        key: "",
        display: "Insights",
        context: "Insights",
        keywords: "statistics daily date comparison trend agents samples efficiency",
    },
    Command {
        id: "insights-hourly-merged",
        title: "Insights: Hourly Resolved Cards",
        key: "",
        display: "Insights",
        context: "Insights",
        keywords: "hourly today chart resolved merged",
    },
    Command {
        id: "insights-hourly-created",
        title: "Insights: Hourly Created Cards",
        key: "",
        display: "Insights",
        context: "Insights",
        keywords: "hourly today chart",
    },
    Command {
        id: "insights-hourly-runs",
        title: "Insights: Hourly Agent Starts",
        key: "",
        display: "Insights",
        context: "Insights",
        keywords: "hourly today chart",
    },
    Command {
        id: "insights-trend-14",
        title: "Insights: Recent 14-day Trend",
        key: "",
        display: "Insights",
        context: "Insights",
        keywords: "daily recent trend chart",
    },
    Command {
        id: "insights-trend-30",
        title: "Insights: Recent 30-day Trend",
        key: "",
        display: "Insights",
        context: "Insights",
        keywords: "daily recent trend chart",
    },
    Command {
        id: "insights-cadence-daily",
        title: "Insights: Daily Velocity & Efficiency Trend",
        key: "",
        display: "Insights",
        context: "Insights",
        keywords: "daily dod comparison velocity efficiency trend",
    },
    Command {
        id: "insights-cadence-weekly",
        title: "Insights: Weekly Velocity & Efficiency Comparison (WoW)",
        key: "",
        display: "Insights",
        context: "Insights",
        keywords: "weekly wow comparison velocity efficiency trend",
    },
    Command {
        id: "insights-toggle-initial-sync",
        title: "Insights: Toggle Exclude GitHub Initial Sync Intake",
        key: "",
        display: "Insights",
        context: "Insights",
        keywords: "initial sync organic created github intake filter",
    },
    Command {
        id: "insights-sort-wip",
        title: "Insights: Sort Projects by Active WIP",
        key: "",
        display: "Insights",
        context: "Insights",
        keywords: "sort projects wip backlog develop review",
    },
    Command {
        id: "insights-sort-runs",
        title: "Insights: Sort Projects by Agent Runs",
        key: "",
        display: "Insights",
        context: "Insights",
        keywords: "sort projects agent runs starts",
    },
    Command {
        id: "insights-range-7d",
        title: "Insights: Show Last 7 Days (7D)",
        key: "",
        display: "Insights",
        context: "Insights",
        keywords: "7d week range window filter statistics",
    },
    Command {
        id: "insights-range-30d",
        title: "Insights: Show Last 30 Days (30D)",
        key: "",
        display: "Insights",
        context: "Insights",
        keywords: "30d month range window filter statistics",
    },
    Command {
        id: "insights-range-90d",
        title: "Insights: Show Last 90 Days (90D)",
        key: "",
        display: "Insights",
        context: "Insights",
        keywords: "90d quarter range window filter statistics",
    },
    Command {
        id: "insights-all-projects",
        title: "Insights: Scope to All Projects",
        key: "",
        display: "Insights",
        context: "Insights",
        keywords: "all projects scope filter global statistics",
    },
    Command {
        id: "insights-current-project",
        title: "Insights: Scope to Current Project",
        key: "",
        display: "Insights",
        context: "Insights",
        keywords: "current project scope filter statistics",
    },
    Command {
        id: "go-workflows",
        title: "Open Project Workflows",
        key: "",
        display: "Project",
        context: "Navigation",
        keywords: "studio stages yaml",
    },
    Command {
        id: "go-schedules",
        title: "Open Scheduled Tasks",
        key: "",
        display: "Schedules",
        context: "Global",
        keywords: "cron timer schedule global host periodic automation",
    },
    Command {
        id: "go-schedules-global",
        title: "Open Global Host Schedules",
        key: "",
        display: "Schedules",
        context: "Global",
        keywords: "cron global host cleanup worktree periodic",
    },
    Command {
        id: "go-schedules-attention",
        title: "Open Schedules Needing Attention",
        key: "",
        display: "Schedules",
        context: "Global",
        keywords: "cron blocked failed attention needs you",
    },
    Command {
        id: "new-scheduled-workflow",
        title: "New Scheduled Task",
        key: "",
        display: "Schedules",
        context: "Schedules",
        keywords: "cron timer schedule workflow create global project periodic",
    },
    Command {
        id: "edit-workflow-agent",
        title: "Edit Workflows with Default Agent",
        key: "",
        display: "Workflows",
        context: "Project",
        keywords: "edit with agent configuration assistant yaml terminal",
    },
    Command {
        id: "set-default-workflow",
        title: "Set Current Project Workflow as Default",
        key: "",
        display: "Workflows",
        context: "Project",
        keywords: "new project template global default workflow reuse save",
    },
    Command {
        id: "reset-default-workflow",
        title: "Reset Default Workflow to Built-in",
        key: "",
        display: "Workflows",
        context: "Preferences",
        keywords: "clear template global default workflow builtin new project",
    },
    Command {
        id: "go-agent-setup",
        title: "Agents · Setup & Enabled Providers",
        key: "",
        display: "Agents",
        context: "Preferences",
        keywords: "agent-setup setup agent onboard enable disable herdr identities default",
    },
    Command {
        id: "go-project-agents",
        title: "Agents · Project Identities & Overrides",
        key: "",
        display: "Project",
        context: "Preferences",
        keywords: "project-agents setup agent identity override roles model effort",
    },
    Command {
        id: "go-settings-agents",
        title: "Agents · Identities & Custom Commands",
        key: "",
        display: "Settings",
        context: "Preferences",
        keywords: "agents identities command argv custom cli model effort defaults",
    },
    Command {
        id: "go-drafts",
        title: "Open Drafts",
        key: "",
        display: "Workspace",
        context: "Navigation",
        keywords: "unsent",
    },
    Command {
        id: "go-archive",
        title: "Open Project Archive",
        key: "",
        display: "Project",
        context: "Navigation",
        keywords: "archived closed",
    },
    Command {
        id: "go-settings",
        title: "Open Settings",
        key: "",
        display: "App",
        context: "Navigation",
        keywords: "preferences config identities",
    },
    Command {
        id: "go-acknowledgements",
        title: "Open Acknowledgements & Licenses",
        key: "",
        display: "Settings",
        context: "Preferences",
        keywords: "about credits open source licenses apache mit herdr sheprd herdr-gpui nsh ghostty",
    },
    Command {
        id: "go-project-location",
        title: "Configure Project Work Location",
        key: "",
        display: "Project",
        context: "Navigation",
        keywords: "directory folder repository path",
    },
    Command {
        id: "go-github-repo",
        title: "Configure Project GitHub Repository",
        key: "",
        display: "GitHub",
        context: "Navigation",
        keywords: "sync repo owner",
    },
    Command {
        id: "link-detected-github-repo",
        title: "Link Detected GitHub Repository",
        key: "",
        display: "GitHub",
        context: "Project",
        keywords: "origin remote auto-detect connect repository binding",
    },
    Command {
        id: "sync-github-repo",
        title: "Sync Project GitHub Repository",
        key: "",
        display: "GitHub",
        context: "Project",
        keywords: "refresh issues pull requests intake",
    },
    Command {
        id: "disconnect-github-repo",
        title: "Disconnect Project GitHub Repository",
        key: "",
        display: "GitHub",
        context: "Project",
        keywords: "unlink remove repository binding",
    },
    Command {
        id: "go-github-account",
        title: "GitHub Account & Access",
        key: "",
        display: "GitHub",
        context: "Preferences",
        keywords: "oauth login token",
    },
    Command {
        id: "new-project",
        title: "Create New Project…",
        key: "",
        display: "Workspace",
        context: "Navigation",
        keywords: "add repository",
    },
    Command {
        id: "switch-project",
        title: "Switch Project",
        key: "",
        display: "Workspace",
        context: "Navigation",
        keywords: "choose project",
    },
    Command {
        id: "popout-card-terminal",
        title: "Pop Out Card Terminal to Terminals",
        key: "",
        display: "Card",
        context: "Card",
        keywords: "full screen expand pty",
    },
    Command {
        id: "open-terminal-diff",
        title: "View Terminal Changes in Git",
        key: "",
        display: "Terminal",
        context: "Terminal",
        keywords: "diff compare branch current directory",
    },
    Command {
        id: "copy-card-branch",
        title: "Copy Card Branch",
        key: "",
        display: "Card",
        context: "Card",
        keywords: "git branch clipboard",
    },
    Command {
        id: "refresh-card-diff",
        title: "Refresh Card Diff",
        key: "",
        display: "Card",
        context: "Card",
        keywords: "git compare branch changes reload",
    },
    Command {
        id: "open-card-diff",
        title: "Open Card Diff in Git",
        key: "",
        display: "Card",
        context: "Card",
        keywords: "git review compare patch changes",
    },
    Command {
        id: "open-pr-diff",
        title: "Open Card PR Diff",
        key: "",
        display: "Card",
        context: "Card",
        keywords: "github pr diff review drafts publish",
    },
    Command {
        id: "agent-review-pr",
        title: "Run Agent PR Review",
        key: "",
        display: "Card",
        context: "Card",
        keywords: "agent reviewer pr diff draft comments",
    },
    Command {
        id: "discard-review-draft",
        title: "Discard Local PR Review Drafts",
        key: "",
        display: "Card",
        context: "Card",
        keywords: "discard clear review draft comments",
    },
    Command {
        id: "advance-card-stage",
        title: "Advance Card to Next Stage",
        key: "",
        display: "Card",
        context: "Card",
        keywords: "move next stage workflow pipeline column develop review",
    },
    Command {
        id: "quick-merge-card",
        title: "Quick Merge Current Card",
        key: "",
        display: "Card",
        context: "Card",
        keywords: "accept deliver merge branch",
    },
    Command {
        id: "retry-auto-merge",
        title: "Retry Auto-Merge",
        key: "",
        display: "Card",
        context: "Card",
        keywords: "deliver queue merge conflict auto-merge merger",
    },
    Command {
        id: "edit-card",
        title: "Edit Card Description",
        key: "",
        display: "Card",
        context: "Card",
        keywords: "task body title",
    },
    Command {
        id: "edit-last-comment",
        title: "Edit Latest Comment",
        key: "",
        display: "Card",
        context: "Card",
        keywords: "modify update message comment discussion github",
    },
    Command {
        id: "delete-last-comment",
        title: "Delete Latest Comment…",
        key: "",
        display: "Card",
        context: "Card",
        keywords: "remove erase trash message comment discussion github",
    },
    Command {
        id: "open-last-github-comment",
        title: "Open Latest GitHub Comment in Browser",
        key: "",
        display: "GitHub",
        context: "Navigation",
        keywords: "jump link external web browser anchor comment discussion pr issue",
    },
    Command {
        id: "open-card-pr",
        title: "Open Card PR on GitHub",
        key: "",
        display: "GitHub",
        context: "Card",
        keywords: "pull request github browser link jump",
    },
    Command {
        id: "open-card-ci",
        title: "Open Card PR CI Checks on GitHub",
        key: "",
        display: "GitHub",
        context: "Card",
        keywords: "ci checks pull request github actions status jump",
    },
    Command {
        id: "refresh-card-pr",
        title: "Refresh Card PR & CI Status",
        key: "",
        display: "GitHub",
        context: "Card",
        keywords: "ci checks pull request refresh sync",
    },
    Command {
        id: "card-workspace-details",
        title: "Toggle Card Workspace Details",
        key: "",
        display: "Card",
        context: "Card",
        keywords: "worktree branch directory",
    },
    Command {
        id: "card-attachments",
        title: "Toggle Card Attachments",
        key: "",
        display: "Card",
        context: "Card",
        keywords: "files images upload",
    },
    Command {
        id: "resolve-card",
        title: "Mark Current Card as Resolved",
        key: "",
        display: "Card",
        context: "Card",
        keywords: "resolve done finish complete close outcome",
    },
    Command {
        id: "reopen-card",
        title: "Reopen Resolved Card",
        key: "",
        display: "Card",
        context: "Card",
        keywords: "reopen unresolve undo restore",
    },
    Command {
        id: "archive-card",
        title: "Archive or Restore Current Card",
        key: "",
        display: "Card",
        context: "Card",
        keywords: "close reopen lifecycle",
    },
    Command {
        id: "toggle-terminal-sidebar",
        title: "Toggle Terminal Sidebar",
        key: "",
        display: "Terminals",
        context: "Terminals",
        keywords: "tree panel hide show",
    },
    Command {
        id: "terminals-collapse-others",
        title: "Terminals: Collapse Other Worktrees",
        key: "",
        display: "Terminals",
        context: "Terminals",
        keywords: "fold collapse worktree project sidebar tree",
    },
    Command {
        id: "terminals-expand-all",
        title: "Terminals: Expand All Worktrees",
        key: "",
        display: "Terminals",
        context: "Terminals",
        keywords: "unfold expand worktree project sidebar tree",
    },
    Command {
        id: "rename-terminal",
        title: "Rename Current Terminal Tab…",
        key: "",
        display: "Terminals",
        context: "Terminals",
        keywords: "label title",
    },
    Command {
        id: "interrupt-terminal",
        title: "Interrupt Active Terminal",
        key: "",
        display: "Terminals",
        context: "Terminals",
        keywords: "stop sigint ctrl-c",
    },
    Command {
        id: "choose-theme",
        title: "Choose Theme",
        key: "",
        display: "Appearance",
        context: "Preferences",
        keywords: "dark light color",
    },
    Command {
        id: "choose-terminal-font",
        title: "Choose Terminal Font…",
        key: "",
        display: "Appearance",
        context: "Preferences",
        keywords: "monospace family size",
    },
    Command {
        id: "toggle-awake",
        title: "Toggle Keep Host Awake",
        key: "",
        display: "System",
        context: "Preferences",
        keywords: "coffee sleep caffeinate power",
    },
    Command {
        id: "refresh-usage",
        title: "Refresh Subscription Usage",
        key: "",
        display: "System",
        context: "Preferences",
        keywords: "quota codex claude limits",
    },
    Command {
        id: "reload-config",
        title: "Reload Configuration",
        key: "",
        display: "System",
        context: "Preferences",
        keywords: "profiles json refresh",
    },
    Command {
        id: "copy-skill-command",
        title: "Copy Canban CLI Skill Install Command",
        key: "",
        display: "Agents",
        context: "Preferences",
        keywords: "npx skills add agent skill canban cli install",
    },
    Command {
        id: "check-herdr",
        title: "Check Herdr Runtime Connection",
        key: "",
        display: "System",
        context: "Preferences",
        keywords: "daemon pty status",
    },
    Command {
        id: "check-updates",
        title: "Check for Application Updates",
        key: "",
        display: "System",
        context: "Preferences",
        keywords: "upgrade version release",
    },
    Command {
        id: "report-issue",
        title: "Report an Issue…",
        key: "",
        display: "Support",
        context: "Preferences",
        keywords: "bug feedback diagnostics",
    },
    Command {
        id: "git-compare",
        title: "Git: Toggle Comparison Controls",
        key: "",
        display: "Git",
        context: "Git",
        keywords: "base compare refs range",
    },
    Command {
        id: "git-related-card",
        title: "Git: Open Related Card",
        key: "",
        display: "Git",
        context: "Git",
        keywords: "worktree path card",
    },
    Command {
        id: "git-fold-file",
        title: "Git: Toggle Selected File Diff",
        key: "",
        display: "Git",
        context: "Git",
        keywords: "collapse expand fold changes",
    },
    Command {
        id: "git-refresh",
        title: "Git: Refresh",
        key: "",
        display: "Git",
        context: "Git",
        keywords: "reload refresh refs log diff",
    },
    Command {
        id: "git-more-files",
        title: "Git: Show More Diff Files",
        key: "",
        display: "Git",
        context: "Git",
        keywords: "diff files load more",
    },
    Command {
        id: "git-fetch",
        title: "Git: Fetch Remote",
        key: "",
        display: "Git",
        context: "Git",
        keywords: "fetch origin remote sync prune",
    },
    Command {
        id: "git-pull",
        title: "Git: Pull (Fast-Forward Only)",
        key: "",
        display: "Git",
        context: "Git",
        keywords: "pull ff-only fast-forward sync update",
    },
    Command {
        id: "git-push",
        title: "Git: Push Current Branch",
        key: "",
        display: "Git",
        context: "Git",
        keywords: "push origin publish upstream",
    },
    Command {
        id: "git-stage-all",
        title: "Git: Stage All Changes",
        key: "",
        display: "Git",
        context: "Git",
        keywords: "stage add working tree index",
    },
    Command {
        id: "git-unstage-all",
        title: "Git: Unstage All Changes",
        key: "",
        display: "Git",
        context: "Git",
        keywords: "unstage reset index",
    },
    Command {
        id: "git-working-tree",
        title: "Git: Open Working Tree & Commit Composer",
        key: "",
        display: "Git",
        context: "Git",
        keywords: "commit amend stage status diff",
    },
];

pub fn execute(ui: Ui, id: &str) {
    if id == "toggle-notifications" || id == "go-overview" {
        ui.quick_open.set(false);
        crate::overview::toggle(ui);
        return;
    }
    crate::utilities::close(ui);
    ui.quick_open.set(false);

    if let Some(key) = id.strip_prefix("git-fold:") {
        crate::review::toggle_file(ui, key.to_owned());
        return;
    }
    if let Some(key) = id.strip_prefix("git-file:") {
        crate::review::focus_file(ui, key.to_owned());
        return;
    }
    if let Some(path) = id.strip_prefix("git-checkout:") {
        crate::review::git_ops::select_checkout(ui, path.to_owned());
        return;
    }
    if let Some(stage_id) = id.strip_prefix("move-card:") {
        if let Some(card) = ui.current_card().filter(|c| c.active()) {
            crate::presenter::move_card(ui, &card.id, stage_id);
        }
        return;
    }
    if let Some(kind) = id.strip_prefix("filter-board-type:") {
        let pid = ui.project.get_untracked();
        crate::github::filters::edit(ui, &pid, |f| f.set_kind(kind));
        ui.navigate("board");
        return;
    }
    if let Some(author) = id.strip_prefix("filter-board-author:") {
        let pid = ui.project.get_untracked();
        crate::github::filters::edit(ui, &pid, |f| {
            f.set_author(if author.is_empty() { "" } else { "author" }, author)
        });
        ui.navigate("board");
        return;
    }
    if let Some(tag) = id.strip_prefix("filter-tag:") {
        let pid = ui.project.get_untracked();
        if !pid.is_empty() {
            crate::tags::toggle_board_filter(ui, &pid, tag);
            ui.navigate("board");
        }
        return;
    }
    if let Some(wt_id) = id.strip_prefix("worktree-shell:") {
        if let Some(wt) = ui
            .snapshot
            .with_untracked(|s| s.worktrees.iter().find(|w| w.id == wt_id).cloned())
        {
            crate::terminals::worktrees::open_shell(ui, &wt);
        }
        return;
    }
    if let Some(wt_id) = id.strip_prefix("worktree-promote:") {
        if ui
            .snapshot
            .with_untracked(|s| s.worktrees.iter().any(|w| w.id == wt_id))
        {
            ui.worktree_promote.set(Some(wt_id.to_owned()));
        }
        return;
    }
    if let Some(wt_id) = id.strip_prefix("worktree-diff:") {
        if let Some(wt) = ui
            .snapshot
            .with_untracked(|s| s.worktrees.iter().find(|w| w.id == wt_id).cloned())
        {
            crate::terminals::worktrees::open_diff(ui, &wt);
        }
        return;
    }
    if let Some(wt_id) = id.strip_prefix("worktree-delete:") {
        if ui
            .snapshot
            .with_untracked(|s| s.worktrees.iter().any(|w| w.id == wt_id))
        {
            crate::terminals::worktrees::request_remove(ui, wt_id.to_owned(), false);
        }
        return;
    }
    if let Some(sched_id) = id.strip_prefix("schedule-open:") {
        let scope = ui.snapshot.with_untracked(|s| {
            s.schedule_states
                .iter()
                .find(|st| st.id == sched_id)
                .map(|st| st.project_id.clone())
        });
        crate::schedules::open(ui, scope, Some(sched_id.to_owned()));
        return;
    }
    if let Some(sched_id) = id.strip_prefix("schedule-run:") {
        crate::workflows::trigger_schedule_by_id(ui, sched_id);
        return;
    }
    if let Some(sched_id) = id.strip_prefix("schedule-toggle:") {
        crate::workflows::toggle_schedule_by_id(ui, sched_id);
        return;
    }

    match id {
        "back" => crate::navigation::go(ui, false),
        "forward" => crate::navigation::go(ui, true),
        "new-card" => ui.new_card(),
        "create-card" if ui.page.get_untracked() == "new-card" => {
            ui.card_submit.update(|n| *n += 1)
        }
        "save-card" => ui.task_submit.update(|n| *n += 1),
        "send-comment" if ui.current_card().is_some() => ui.comment_submit.update(|n| *n += 1),
        "quick-open" => ui.show_search(false),
        "shortcuts" => {
            ui.utility_popup.set(String::new());
            ui.project_picker.set(false);
            ui.search_query.set(">".into());
            ui.quick_open.set(true);
        }
        "mark-all-read" => crate::overview::mark_all_read(ui, None),
        "go-board" => ui.open_board(),
        "clear-board-filters" => {
            crate::github::filters::reset(ui);
            ui.navigate("board");
        }
        "filter-board-mine" => {
            crate::github::filters::mine(ui);
            ui.navigate("board");
        }
        "filter-board-more" => {
            ui.navigate("board");
            crate::github::filters::open_more(ui, ui.project.get_untracked());
        }
        "go-terminals" => ui.navigate("terminals"),
        "go-review" => crate::review::open_compare(ui),
        "go-github" => ui.navigate("github"),
        "go-insights" => ui.navigate("insights"),
        "insights-today" => {
            crate::insights::intent(ui, "today");
            ui.navigate("insights");
        }
        "insights-previous-day" => {
            crate::insights::intent(ui, "previous-day");
            ui.navigate("insights");
        }
        "insights-next-day" => {
            crate::insights::intent(ui, "next-day");
            ui.navigate("insights");
        }
        "insights-apply-date" => {
            crate::insights::intent(ui, "apply-date");
            ui.navigate("insights");
        }
        "insights-compare-previous" => {
            crate::insights::intent(ui, "compare-previous");
            ui.navigate("insights");
        }
        "insights-compare-week" => {
            crate::insights::intent(ui, "compare-week");
            ui.navigate("insights");
        }
        "insights-metric-merged" => {
            crate::insights::intent(ui, "metric-merged");
            ui.navigate("insights");
        }
        "insights-metric-created" => {
            crate::insights::intent(ui, "metric-created");
            ui.navigate("insights");
        }
        "insights-metric-runs" => {
            crate::insights::intent(ui, "metric-runs");
            ui.navigate("insights");
        }
        "insights-efficiency" => {
            crate::insights::intent(ui, "efficiency");
            ui.navigate("insights");
        }
        "insights-samples-cycle" => {
            crate::insights::intent(ui, "samples-cycle");
            ui.navigate("insights");
        }
        "insights-samples-queue" => {
            crate::insights::intent(ui, "samples-queue");
            ui.navigate("insights");
        }
        "insights-samples-next" => {
            crate::insights::intent(ui, "samples-next");
            ui.navigate("insights");
        }
        "insights-samples-previous" => {
            crate::insights::intent(ui, "samples-previous");
            ui.navigate("insights");
        }
        "insights-group-provider" => {
            crate::insights::intent(ui, "group-provider");
            ui.navigate("insights");
        }
        "insights-group-profile" => {
            crate::insights::intent(ui, "group-profile");
            ui.navigate("insights");
        }
        "insights-sort-created" => {
            crate::insights::intent(ui, "sort-created");
            ui.navigate("insights");
        }
        "insights-sort-merged" => {
            crate::insights::intent(ui, "sort-merged");
            ui.navigate("insights");
        }
        "insights-cards-created" => {
            crate::insights::intent(ui, "cards-created");
            ui.navigate("insights");
        }
        "insights-cards-merged" => {
            crate::insights::intent(ui, "cards-merged");
            ui.navigate("insights");
        }
        "insights-cards-close" => {
            crate::insights::intent(ui, "cards-close");
            ui.navigate("insights");
        }
        "insights-hourly-merged" => {
            crate::insights::intent(ui, "hourly-merged");
            ui.navigate("insights");
        }
        "insights-hourly-created" => {
            crate::insights::intent(ui, "hourly-created");
            ui.navigate("insights");
        }
        "insights-hourly-runs" => {
            crate::insights::intent(ui, "hourly-runs");
            ui.navigate("insights");
        }
        "insights-trend-14" => {
            crate::insights::intent(ui, "trend-14");
            ui.navigate("insights");
        }
        "insights-trend-30" => {
            crate::insights::intent(ui, "trend-30");
            ui.navigate("insights");
        }
        "insights-cadence-daily" => {
            crate::insights::intent(ui, "cadence-daily");
            ui.navigate("insights");
        }
        "insights-cadence-weekly" => {
            crate::insights::intent(ui, "cadence-weekly");
            ui.navigate("insights");
        }
        "insights-toggle-initial-sync" => {
            crate::insights::intent(ui, "toggle-initial-sync");
            ui.navigate("insights");
        }
        "insights-sort-wip" => {
            crate::insights::intent(ui, "sort-wip");
            ui.navigate("insights");
        }
        "insights-sort-runs" => {
            crate::insights::intent(ui, "sort-runs");
            ui.navigate("insights");
        }
        "insights-range-7d" => {
            crate::insights::set_range(ui, crate::insights::model::RangeWindow::Days7);
            ui.navigate("insights");
        }
        "insights-range-30d" => {
            crate::insights::set_range(ui, crate::insights::model::RangeWindow::Days30);
            ui.navigate("insights");
        }
        "insights-range-90d" => {
            crate::insights::set_range(ui, crate::insights::model::RangeWindow::Days90);
            ui.navigate("insights");
        }
        "insights-all-projects" => {
            ui.insights.scope_project.set(String::new());
            ui.navigate("insights");
        }
        "insights-current-project" => {
            let pid = ui.project.get_untracked();
            if !pid.is_empty() {
                ui.insights.scope_project.set(pid);
                ui.navigate("insights");
            } else {
                ui.notify("Select a Project first");
            }
        }
        "go-workflows" => {
            ui.workflows.section.set("workflows".into());
            ui.navigate("workflows");
        }
        "go-schedules" => crate::schedules::open(ui, None, None),
        "go-schedules-global" => {
            crate::schedules::open(ui, Some(canban_types::GLOBAL_PROJECT_ID.into()), None)
        }
        "go-schedules-attention" => {
            crate::schedules::open(ui, None, None);
            ui.schedules.status_filter.set("attention".into());
        }
        "new-scheduled-workflow" => crate::schedules::new_scheduled_workflow(ui),
        "edit-workflow-agent" => crate::workflows::edit_agent(ui),
        "set-default-workflow" => crate::workflows::set_default_workflow(ui),
        "reset-default-workflow" => crate::workflows::reset_default_workflow(ui),
        "go-project-agents" => {
            if ui.current_project().is_some() {
                ui.navigate("project-agents");
            } else {
                ui.navigate("agent-setup");
            }
        }
        "go-agent-setup" => ui.navigate("agent-setup"),
        "go-settings-agents" => ui.open_settings_section("agents"),
        "go-drafts" => ui.navigate("drafts"),
        "go-archive" => ui.navigate("archive"),
        "go-settings" => ui.navigate("settings"),
        "go-hosts" => ui.open_settings_section("hosts"),
        "go-diagnostics" => {
            let target = ui.active_project_host();
            crate::hosts::open_host_settings(ui, target, "diagnostics");
        }
        "toggle-hosts" => crate::hosts::toggle_popover(ui),
        "connect-machine" => crate::hosts::open_connect_modal(ui),
        "reconnect-hosts" => crate::hosts::reconnect_host(ui, None),
        "go-acknowledgements" => ui.open_settings_section("acknowledgements"),
        "go-project-location" => {
            if ui.current_project().is_some() {
                ui.navigate("project-location");
            } else {
                ui.navigate("new-project");
            }
        }
        "go-github-repo" => {
            if ui.current_project().is_some() {
                ui.navigate("github-repository");
            } else {
                ui.navigate("github");
            }
        }
        "link-detected-github-repo" => {
            if let Some(p) = ui.current_project() {
                if let Some(det) = p.detected_github {
                    let delivery = serde_json::to_value(&p.delivery_mode)
                        .ok()
                        .and_then(|v| v.as_str().map(str::to_owned))
                        .unwrap_or_else(|| "local".into());
                    dispatch(
                        ui,
                        "project.github",
                        json!({
                            "project_id": p.id,
                            "repository": det.repository,
                            "intake": "off",
                            "pr_intake": "off",
                            "authors": [],
                            "labels": [],
                            "include_drafts": false,
                            "paused": false,
                            "delivery_mode": delivery,
                        }),
                    );
                } else {
                    ui.navigate("github-repository");
                }
            } else {
                ui.notify("Select or create a Project first");
            }
        }
        "sync-github-repo" => {
            if let Some(p) = ui.current_project() {
                if p.github.is_some() {
                    dispatch(ui, "github.sync.request", json!({"project_id": p.id}));
                } else {
                    ui.navigate("github-repository");
                }
            } else {
                ui.notify("Select or create a Project first");
            }
        }
        "disconnect-github-repo" => {
            if let Some(p) = ui.current_project() {
                if p.github.is_some() {
                    dispatch(ui, "github.source.disconnect", json!({"project_id": p.id}));
                } else {
                    ui.notify("Current Project has no linked GitHub repository");
                }
            } else {
                ui.notify("Select or create a Project first");
            }
        }
        "go-github-account" => ui.navigate("github-account"),
        "new-project" => ui.navigate("new-project"),
        "switch-project" => ui.show_search(true),
        "toggle-card-terminal" => {
            if ui.page.get_untracked() == "card" && !ui.card.get_untracked().is_empty() {
                crate::card::terminal::toggle(ui);
            } else if let Some(c) = ui.current_card() {
                crate::terminals::open_card_quick(ui, &c.id);
            } else {
                ui.notify("Open a Card first to toggle its Live Terminal");
            }
        }
        "popout-card-terminal" => {
            if !ui.card_id().is_empty() {
                crate::card::terminal::pop_out(ui);
            } else {
                ui.navigate("terminals");
            }
        }
        "open-terminal-diff" => crate::terminals::worktrees::open_current_diff(ui),
        "copy-card-branch" => crate::card::changes::copy_branch(ui),
        "refresh-card-diff" => crate::card::changes::refresh(ui),
        "open-card-diff" => crate::card::changes::open(ui),
        "open-pr-diff" => {
            if let Some(c) = ui.current_card() {
                crate::review::open_card_pr(ui, &c.id);
            } else {
                ui.notify("Select a Card first");
            }
        }
        "agent-review-pr" => {
            if let Some(c) = ui.current_card() {
                if c.active()
                    && !c.resolved()
                    && (crate::review::card_has_pr(&c)
                        || c.worktree.is_some()
                        || c.branch.is_some())
                {
                    crate::review::start_agent_review(ui, &c.id, None, None);
                } else {
                    ui.notify("Current Card is not eligible for Agent PR review");
                }
            } else {
                ui.notify("Select a Card first");
            }
        }
        "discard-review-draft" => {
            if let Some(c) = ui.current_card() {
                crate::review::discard_review_draft(ui, &c.id);
            } else {
                ui.notify("Select a Card first");
            }
        }
        "advance-card-stage" => crate::card::advance_stage(ui),
        "quick-merge-card" => {
            if let Some(c) = ui.current_card() {
                let project = ui.current_project();
                if crate::card::quick_merge_eligible(&c, project.as_ref()) {
                    crate::card::quick_merge_start(ui, c);
                } else {
                    ui.notify("Current Card is not eligible for local quick merge");
                }
            } else {
                ui.notify("Select a Card first");
            }
        }
        "retry-auto-merge" => {
            if let Some(c) = ui.current_card() {
                let (has_deliver_in_project, active_non_deliver) =
                    ui.snapshot.with_untracked(|s| {
                        let has_deliver = s.workflow_revisions.iter().any(|w| {
                            w.project_id == c.project_id
                                && w.definition.steps.values().any(|st| {
                                    matches!(st, canban_types::WorkflowStep::Deliver { .. })
                                })
                        });
                        let non_deliver = s
                            .workflow_runs
                            .iter()
                            .any(|r| r.card_id == c.id && r.active() && !r.step_is_deliver());
                        (has_deliver, non_deliver)
                    });
                if !c.active() || c.resolved() || c.worktree.is_none() || !has_deliver_in_project {
                    ui.notify("Current Card cannot retry auto-merge");
                } else if active_non_deliver {
                    ui.notify("Card workflow is currently active at another step");
                } else {
                    dispatch(
                        ui,
                        "workflow.deliver",
                        json!({"card_id": c.id, "revision": c.revision}),
                    );
                }
            } else {
                ui.notify("Select a Card first");
            }
        }
        "edit-card" => {
            if let Some(c) = ui.current_card() {
                let key = format!("card:{}", c.id);
                let mut d = ui
                    .drafts
                    .with(|m| m.get(&key).cloned())
                    .unwrap_or_else(|| crate::drafts::Draft::card(&c));
                d.editing = !d.editing;
                crate::drafts::put(ui, key, d);
            }
        }
        "edit-last-comment" => {
            if ui.page.get_untracked() == "github" && ui.github.reference.get_untracked().is_some()
            {
                let sec = ui.github.section.get_untracked();
                if matches!(sec.as_str(), "discussion" | "threads" | "reviews") {
                    let my_login = crate::github::active_login_untracked(ui);
                    let target = ui.github.detail.with_untracked(|det| {
                        det["data"].as_array().and_then(|arr| {
                            arr.iter().rev().find_map(|entry| {
                                let cid = entry["id"].as_u64().unwrap_or(0);
                                let author = entry["user"]["login"].as_str().unwrap_or("");
                                (cid > 0
                                    && my_login
                                        .as_deref()
                                        .is_some_and(|me| author.eq_ignore_ascii_case(me)))
                                .then(|| (cid, entry["body"].as_str().unwrap_or("").to_owned()))
                            })
                        })
                    });
                    if let Some((cid, body)) = target {
                        batch(move || {
                            ui.github.edit_error.set(String::new());
                            ui.github.edit_draft.set(body);
                            ui.github.editing_comment.set(Some((sec, cid)));
                        });
                    } else {
                        ui.notify("No comments authored by you in this GitHub section");
                    }
                } else {
                    ui.notify("Open Discussion, Threads, or Reviews to edit a GitHub comment");
                }
            } else if let Some(c) = ui.current_card().filter(|c| c.active()) {
                let latest = ui.snapshot.with_untracked(|s| {
                    s.events
                        .iter()
                        .filter(|e| {
                            e.card_id == c.id && e.kind == "comment.created" && e.actor == "user"
                        })
                        .max_by_key(|e| e.sequence)
                        .map(|e| (e.id.clone(), e.sequence))
                });
                if let Some((eid, seq)) = latest {
                    if ui.page.get_untracked() != "card"
                        && !(ui.page.get_untracked() == "review"
                            && ui.review_state.card_drawer_open.get_untracked())
                    {
                        ui.open_card(c.id);
                    }
                    ui.card_event.set(Some(seq));
                    ui.deleting_comment.set(None);
                    ui.editing_comment.set(Some(eid));
                } else {
                    ui.notify("No user comments to edit on this Card");
                }
            } else {
                ui.notify("Open an active Card or GitHub conversation first");
            }
        }
        "delete-last-comment" => {
            if ui.page.get_untracked() == "github" && ui.github.reference.get_untracked().is_some()
            {
                let sec = ui.github.section.get_untracked();
                if matches!(sec.as_str(), "discussion" | "threads") {
                    let my_login = crate::github::active_login_untracked(ui);
                    let target = ui.github.detail.with_untracked(|det| {
                        det["data"].as_array().and_then(|arr| {
                            arr.iter().rev().find_map(|entry| {
                                let cid = entry["id"].as_u64().unwrap_or(0);
                                let author = entry["user"]["login"].as_str().unwrap_or("");
                                (cid > 0
                                    && my_login
                                        .as_deref()
                                        .is_some_and(|me| author.eq_ignore_ascii_case(me)))
                                .then_some(cid)
                            })
                        })
                    });
                    if let Some(cid) = target {
                        batch(move || {
                            ui.github.delete_error.set(String::new());
                            ui.github.editing_comment.set(None);
                            ui.github.deleting_comment.set(Some((sec, cid)));
                        });
                    } else {
                        ui.notify("No comments authored by you in this GitHub section");
                    }
                } else if sec == "reviews" {
                    ui.notify("GitHub does not support deleting submitted pull request reviews");
                } else {
                    ui.notify("Open Discussion or Review threads to delete a GitHub comment");
                }
            } else if ui.page.get_untracked() == "review"
                && ui.review_state.mode.get_untracked() == "github_pr"
            {
                let my_login = crate::github::active_login_untracked(ui);
                let target = ui.review_state.compare_data.with_untracked(|cd| {
                    cd["threads"].as_array().and_then(|arr| {
                        arr.iter().rev().find_map(|entry| {
                            let cid = entry["id"].as_u64().unwrap_or(0);
                            let author = entry["user"]["login"].as_str().unwrap_or("");
                            (cid > 0
                                && my_login
                                    .as_deref()
                                    .is_some_and(|me| author.eq_ignore_ascii_case(me)))
                            .then_some(cid)
                        })
                    })
                });
                if let Some(cid) = target {
                    batch(move || {
                        ui.review_state.remote_delete_error.set(String::new());
                        ui.review_state.editing_remote_comment.set(None);
                        ui.review_state.deleting_remote_comment.set(Some(cid));
                    });
                } else {
                    ui.notify("No inline PR comments authored by you on this diff");
                }
            } else if let Some(c) = ui.current_card().filter(|c| c.active()) {
                let latest = ui.snapshot.with_untracked(|s| {
                    s.events
                        .iter()
                        .filter(|e| {
                            e.card_id == c.id && e.kind == "comment.created" && e.actor == "user"
                        })
                        .max_by_key(|e| e.sequence)
                        .map(|e| (e.id.clone(), e.sequence))
                });
                if let Some((eid, seq)) = latest {
                    if ui.page.get_untracked() != "card"
                        && !(ui.page.get_untracked() == "review"
                            && ui.review_state.card_drawer_open.get_untracked())
                    {
                        ui.open_card(c.id);
                    }
                    ui.card_event.set(Some(seq));
                    ui.editing_comment.set(None);
                    ui.deleting_comment.set(Some(eid));
                } else {
                    ui.notify("No user comments to delete on this Card");
                }
            } else {
                ui.notify("Open an active Card or GitHub conversation first");
            }
        }
        "open-last-github-comment" => {
            if ui.page.get_untracked() == "github"
                && let Some(r) = ui.github.reference.get_untracked()
            {
                let sec = ui.github.section.get_untracked();
                if matches!(sec.as_str(), "discussion" | "threads" | "reviews") {
                    let url = ui.github.detail.with_untracked(|det| {
                        det["data"].as_array().and_then(|arr| {
                            arr.iter().rev().find_map(|entry| {
                                crate::github::comment_web_url(entry, Some(r.url.as_str()), &sec)
                            })
                        })
                    });
                    if let Some(url) = url {
                        crate::github::open_url(ui, url);
                    } else {
                        ui.notify("No GitHub comments in this section");
                    }
                } else {
                    crate::github::open_url(ui, r.url);
                }
            } else if ui.page.get_untracked() == "review"
                && let Some(pr) = ui.review_state.pr_target.get_untracked()
            {
                let base_url = pr["url"]
                    .as_str()
                    .filter(|u| !u.is_empty())
                    .map(str::to_owned)
                    .or_else(|| {
                        let repo = pr["repository"].as_str().unwrap_or("");
                        let num = pr["number"].as_u64().unwrap_or(0);
                        (!repo.is_empty() && num > 0)
                            .then(|| format!("https://github.com/{repo}/pull/{num}"))
                    });
                let url = ui.review_state.compare_data.with_untracked(|cd| {
                    cd["threads"].as_array().and_then(|arr| {
                        arr.iter().rev().find_map(|entry| {
                            crate::github::comment_web_url(entry, base_url.as_deref(), "threads")
                        })
                    })
                });
                if let Some(url) = url.or(base_url) {
                    crate::github::open_url(ui, url);
                } else {
                    ui.notify("No GitHub pull request link available");
                }
            } else if let Some(c) = ui.current_card()
                && let Some(src) = c.github_issue()
            {
                crate::github::open_url(ui, src.url.clone());
            } else {
                ui.notify("Open a GitHub conversation or GitHub Card first");
            }
        }
        "open-card-pr" => {
            if let Some(c) = ui.current_card()
                && let Some(pr_view) = ui.snapshot.with_untracked(|s| {
                    ui.index
                        .with_untracked(|idx| crate::components::pr_status::from_card(s, idx, &c))
                })
            {
                crate::github::open_url(ui, pr_view.pr_url);
            } else {
                ui.notify("This Card has no linked GitHub PR");
            }
        }
        "open-card-ci" => {
            if let Some(c) = ui.current_card()
                && let Some(pr_view) = ui.snapshot.with_untracked(|s| {
                    ui.index
                        .with_untracked(|idx| crate::components::pr_status::from_card(s, idx, &c))
                })
                && let Some(url) = pr_view.checks_url()
            {
                crate::github::open_url(ui, url);
            } else {
                ui.notify("This Card has no linked GitHub PR CI checks");
            }
        }
        "refresh-card-pr" => {
            if let Some(c) = ui.current_card() {
                dispatch(ui, "github.pr.refresh", json!({ "card_id": c.id }));
            } else {
                ui.notify("Open a Card first to refresh its PR");
            }
        }
        "card-workspace-details" => {
            if let Some(c) = ui.current_card() {
                let key = format!("card:{}", c.id);
                let mut d = ui
                    .drafts
                    .with(|m| m.get(&key).cloned())
                    .unwrap_or_else(|| crate::drafts::Draft::card(&c));
                d.workspace_details = !d.workspace_details;
                crate::drafts::put(ui, key, d);
            }
        }
        "card-attachments" => {
            if let Some(c) = ui.current_card() {
                let key = format!("card:{}", c.id);
                let mut d = ui
                    .drafts
                    .with(|m| m.get(&key).cloned())
                    .unwrap_or_else(|| crate::drafts::Draft::card(&c));
                d.details = !d.details;
                crate::drafts::put(ui, key, d);
            }
        }
        "resolve-card" => {
            if let Some(c) = ui.current_card() {
                if !c.active() {
                    ui.notify("Restore the Card before resolving");
                } else if c.resolved() {
                    ui.notify("Card is already resolved");
                } else {
                    crate::card::resolve_start(ui, c);
                }
            } else {
                ui.notify("Select a Card first");
            }
        }
        "reopen-card" => {
            if let Some(c) = ui.current_card() {
                if !c.active() {
                    ui.notify("Restore the Card before reopening");
                } else if !c.resolved() {
                    ui.notify("Card is not resolved");
                } else if c.merged() {
                    ui.notify("Cards with merged code cannot be reopened");
                } else {
                    crate::card::resolve_reopen(ui, c);
                }
            } else {
                ui.notify("Select a Card first");
            }
        }
        "archive-card" => {
            if let Some(c) = ui.current_card() {
                let inactive = c.archived_at.is_some() || c.removed_at.is_some();
                crate::card_actions::apply(
                    ui,
                    c,
                    if inactive {
                        crate::card_actions::Action::Restore
                    } else {
                        crate::card_actions::Action::Archive
                    },
                );
            }
        }
        "new-terminal" => {
            if ui.page.get_untracked() == "card"
                && ui.card_terminal_open.get_untracked()
                && let Some(c) = ui.current_card()
            {
                crate::terminals::create_in_card(ui, c.project_id, c.id, false);
                ui.terminal_focus.update(|v| *v += 1);
            } else if ui.page.get_untracked() == "terminals" {
                crate::terminals::new_tab(ui);
                ui.terminal_focus.update(|v| *v += 1);
            } else {
                crate::terminals::open_global(ui);
            }
        }
        "new-worktree" => {
            let pid = ui.project.get_untracked();
            let target = if pid.is_empty() {
                ui.snapshot
                    .with_untracked(|s| s.projects.first().map(|p| p.id.clone()))
                    .unwrap_or_default()
            } else {
                pid
            };
            if target.is_empty() {
                ui.navigate("new-project");
            } else {
                ui.navigate("terminals");
                ui.worktree_modal.set(Some(target));
            }
        }
        "close-terminal" if ui.page.get_untracked() == "terminals" => {
            crate::terminals::close_tab(ui, &ui.terminal_selection.get_untracked())
        }
        "close-terminal"
            if ui.page.get_untracked() == "card" && ui.card_terminal_open.get_untracked() =>
        {
            let cid = ui.card_id();
            if let Some(entry) = crate::card::terminal::select_entry_for_card(ui, &cid) {
                crate::terminals::close_entry(ui, &entry.saved.id);
            }
        }
        "next-tab" => {
            if ui.page.get_untracked() == "card" && ui.card_terminal_open.get_untracked() {
                crate::terminals::cycle(ui, true);
            } else {
                ui.navigate("terminals");
                crate::terminals::cycle(ui, true);
            }
        }
        "previous-tab" => {
            if ui.page.get_untracked() == "card" && ui.card_terminal_open.get_untracked() {
                crate::terminals::cycle(ui, false);
            } else {
                ui.navigate("terminals");
                crate::terminals::cycle(ui, false);
            }
        }
        id if id.starts_with("terminal-tab-") => {
            if let Some(n) = id
                .strip_prefix("terminal-tab-")
                .and_then(|s| s.parse::<usize>().ok())
                .filter(|&n| (1..=9).contains(&n))
            {
                if !(ui.page.get_untracked() == "card" && ui.card_terminal_open.get_untracked()) {
                    ui.navigate("terminals");
                }
                crate::terminals::select_tab_index(ui, n - 1);
            }
        }
        "toggle-terminal-sidebar" => {
            ui.navigate("terminals");
            ui.terminal_sidebar.update(|v| *v = !*v);
        }
        "terminals-collapse-others" => {
            ui.navigate("terminals");
            ui.terminal_sidebar.set(true);
            crate::terminals::tree::collapse_other_worktrees(ui);
        }
        "terminals-expand-all" => {
            ui.navigate("terminals");
            ui.terminal_sidebar.set(true);
            crate::terminals::tree::expand_all_worktrees(ui);
        }
        "rename-terminal" => {
            if ui.page.get_untracked() == "card" && ui.card_terminal_open.get_untracked() {
                let cid = ui.card_id();
                if let Some(entry) = crate::card::terminal::select_entry_for_card(ui, &cid) {
                    ui.terminal_rename.set(Some(entry.saved.id));
                }
            } else {
                let sel = ui.terminal_selection.get_untracked();
                if !sel.is_empty() {
                    ui.navigate("terminals");
                    ui.terminal_rename.set(Some(sel));
                }
            }
        }
        "interrupt-terminal" => {
            let entry =
                if ui.page.get_untracked() == "card" && ui.card_terminal_open.get_untracked() {
                    let cid = ui.card_id();
                    crate::card::terminal::select_entry_for_card(ui, &cid)
                } else {
                    crate::terminals::current(ui)
                };
            if let Some(e) = entry {
                if e.agent
                    .as_ref()
                    .and_then(|a| a.agent())
                    .is_some_and(|a| !a.finished() && a.terminal.is_some())
                {
                    dispatch(ui, "agent.interrupt", json!({"terminal_id": e.saved.id}));
                } else if !e.ended() && e.binding().is_some() {
                    dispatch(ui, "terminal.interrupt", json!({"terminal_id": e.saved.id}));
                }
            }
        }
        "choose-theme" => crate::utilities::open(ui, "theme"),
        "choose-terminal-font" => crate::utilities::open(ui, "terminal-font"),
        "toggle-awake" => crate::utilities::toggle_awake(ui),
        "refresh-usage" => crate::utilities::usage(ui, crate::utilities::host(ui), true),
        "reload-config" => dispatch(ui, "config.reload", json!({})),
        "copy-skill-command" => crate::agents_setup::copy_skill_command(ui),
        "check-herdr" => dispatch(ui, "herdr.status", json!({})),
        "check-updates" => crate::updates::check(ui),
        "report-issue" => crate::utilities::report::open(ui),
        "git-compare" => crate::review::toggle_comparison(ui),
        "git-related-card" => crate::review::open_related_card(ui),
        "git-fold-file" => {
            let key = ui.review_state.selected_file.get_untracked();
            if !key.is_empty() {
                crate::review::toggle_file(ui, key);
            }
        }
        "git-refresh" => crate::review::refresh(ui),
        "git-more-files" => crate::review::show_more_files(ui),
        "git-fetch" => {
            if ui.page.get_untracked() != "review" {
                crate::review::open_compare(ui);
            }
            crate::review::git_ops::run_fetch(ui);
        }
        "git-pull" => {
            if ui.page.get_untracked() != "review" {
                crate::review::open_compare(ui);
            }
            crate::review::git_ops::run_pull(ui);
        }
        "git-push" => {
            if ui.page.get_untracked() != "review" {
                crate::review::open_compare(ui);
            }
            crate::review::git_ops::run_push(ui);
        }
        "git-stage-all" => {
            if ui.page.get_untracked() != "review" {
                crate::review::open_compare(ui);
            }
            crate::review::select_git_pair(ui, "HEAD", "UNCOMMITTED");
            crate::review::git_ops::run_stage(ui, Vec::new(), true);
        }
        "git-unstage-all" => {
            if ui.page.get_untracked() != "review" {
                crate::review::open_compare(ui);
            }
            crate::review::select_git_pair(ui, "HEAD", "UNCOMMITTED");
            crate::review::git_ops::run_unstage(ui, Vec::new(), true);
        }
        "git-working-tree" => {
            crate::review::open_compare(ui);
            crate::review::select_git_pair(ui, "HEAD", "UNCOMMITTED");
        }
        _ => {}
    }
}

pub fn bindings(ui: Ui) {
    crate::navigation::watch(ui);
    crate::terminals::watch(ui);
    effect(move || {
        if ui.page.get() == "new-card" && !ui.quick_open.get() && ui.utility_popup.get().is_empty()
        {
            on_key_with(
                CREATE_CARD.key,
                KeyOptions {
                    when: When::Always,
                    ..Default::default()
                },
                move |_| {
                    ui.card_submit.update(|n| *n += 1);
                },
            );
        }
    });
    effect(move || {
        if ui.page.get() == "card"
            && !ui.card.get().is_empty()
            && !ui.quick_open.get()
            && ui.utility_popup.get().is_empty()
            && !ui.native_modal.get()
        {
            on_key_with(
                shortcuts::CARD_TERMINAL_KEY,
                KeyOptions {
                    when: When::Always,
                    ..Default::default()
                },
                move |_| crate::card::terminal::toggle(ui),
            );
        }
    });
    for command in CATALOG
        .iter()
        .filter(|c| c.context == "Global" && !c.key.is_empty())
    {
        on_key_with(
            command.key,
            KeyOptions {
                when: When::Always,
                ..Default::default()
            },
            move |_| {
                if command.id == "new-terminal"
                    && (ui.quick_open.get_untracked()
                        || !ui.utility_popup.get_untracked().is_empty()
                        || ui.native_modal.get_untracked()
                        || ui.terminal_rename.get_untracked().is_some()
                        || ui.terminal_close_confirm.get_untracked().is_some()
                        || ui.worktree_modal.get_untracked().is_some()
                        || ui.worktree_promote.get_untracked().is_some()
                        || ui.worktree_remove_confirm.get_untracked().is_some())
                {
                    return;
                }
                execute(ui, command.id);
            },
        );
    }
    #[cfg(not(target_os = "macos"))]
    {
        for (extra_key, id) in [
            ("ctrl+shift+p", "shortcuts"),
            ("alt+left", "back"),
            ("alt+right", "forward"),
        ] {
            let when = if extra_key.starts_with("alt+") {
                When::NotEditing
            } else {
                When::Always
            };
            on_key_with(
                extra_key,
                KeyOptions {
                    when,
                    ..Default::default()
                },
                move |_| execute(ui, id),
            );
        }
    }
    // Plain `/` opens the command palette (`>` prefix) whenever focus is not in a text input or hosted terminal.
    on_key_with(
        "/",
        KeyOptions {
            when: When::NotEditing,
            ..Default::default()
        },
        move |_| {
            if !ui.quick_open.get_untracked()
                && ui.utility_popup.get_untracked().is_empty()
                && !ui.native_modal.get_untracked()
                && ui.terminal_rename.get_untracked().is_none()
                && ui.terminal_close_confirm.get_untracked().is_none()
                && ui.worktree_modal.get_untracked().is_none()
                && ui.worktree_promote.get_untracked().is_none()
                && ui.worktree_remove_confirm.get_untracked().is_none()
            {
                ui.project_picker.set(false);
                ui.search_query.set(">".into());
                ui.quick_open.set(true);
            }
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_ids_are_unique_and_shortcuts_parse() {
        let mut seen = std::collections::BTreeSet::new();
        for cmd in CATALOG {
            assert!(
                seen.insert(cmd.id),
                "duplicate command id in CATALOG: {}",
                cmd.id
            );
            if !cmd.key.is_empty() {
                nsh_core::desktop::Shortcut::parse(cmd.key, false).unwrap();
            }
        }
        let new_term = CATALOG
            .iter()
            .find(|c| c.id == "new-terminal")
            .expect("new-terminal registered");
        assert_eq!(new_term.context, "Global");
        assert_eq!(new_term.key, shortcuts::NEW_TERMINAL_KEY);
        assert!(
            !crate::terminals::TAB_KEYS
                .iter()
                .any(|(k, _)| *k == shortcuts::NEW_TERMINAL_KEY)
        );
        #[cfg(not(target_os = "macos"))]
        {
            for cmd in CATALOG {
                assert!(
                    !cmd.display.contains('⌘')
                        && !cmd.display.contains('⇧')
                        && !cmd.display.contains('⌥'),
                    "non-macOS shortcut label must not contain Apple symbols: {} -> {}",
                    cmd.id,
                    cmd.display
                );
            }
        }
        assert!(CATALOG.iter().any(|c| c.id == "copy-skill-command"));
        assert!(CATALOG.iter().any(|c| c.id == "edit-last-comment"));
        assert!(CATALOG.iter().any(|c| c.id == "delete-last-comment"));
        assert!(CATALOG.iter().any(|c| c.id == "open-last-github-comment"));
        assert!(CATALOG.iter().any(|c| c.id == "advance-card-stage"));
        assert!(CATALOG.iter().any(|c| c.id == "retry-auto-merge"));
        assert!(CATALOG.iter().any(|c| c.id == "go-insights"));
        assert!(CATALOG.iter().any(|c| c.id == "insights-range-30d"));
        let go_git = CATALOG.iter().find(|c| c.id == "go-review").unwrap();
        assert_eq!(go_git.title, "Go to Git");
        assert_eq!(go_git.display, "Git");
        assert!(go_git.keywords.contains("diff"));
    }

    #[test]
    fn linux_shortcut_matrix_parses_cleanly_and_avoids_pty_control_collisions() {
        let linux_bindings = [
            "ctrl+enter",
            "ctrl+s",
            "ctrl+shift+i",
            "ctrl+shift+n",
            "ctrl+shift+o",
            "ctrl+shift+/",
            "ctrl+shift+p",
            "ctrl+shift+j",
            "ctrl+shift+t",
            "ctrl+shift+w",
            "ctrl+shift+]",
            "ctrl+shift+[",
            "ctrl+alt+[",
            "ctrl+alt+]",
            "alt+1",
            "alt+2",
            "alt+3",
            "alt+4",
            "alt+5",
            "alt+6",
            "alt+7",
            "alt+8",
            "alt+9",
        ];
        let pty_ctrl_hazards = [
            "ctrl+i", "ctrl+j", "ctrl+w", "ctrl+n", "ctrl+o", "ctrl+t", "ctrl+[", "ctrl+]",
        ];
        for key in linux_bindings {
            nsh_core::keymap::Keystroke::parse(key, nsh_core::platform::Os::Linux)
                .unwrap_or_else(|e| panic!("Linux keystroke `{key}` failed to parse: {e:?}"));
            assert!(
                !pty_ctrl_hazards.contains(&key),
                "Linux global/terminal binding `{key}` collides with PTY control codes or GTK window close"
            );
        }
    }
}
