use crate::presenter::*;
use nsh_core::prelude::*;

pub fn screen(ui: Ui, page: &str) -> AnyElement {
    match page {
        "agent-setup" => crate::agents_setup::onboarding_screen(ui).into_element(),
        "project-agents" => crate::agents_setup::project_agents_screen(ui).into_element(),
        "workflows" => crate::workflows::screen(ui).into_element(),
        "project-location" => crate::projects::project_location(ui).into_element(),
        "new-project" => crate::projects::new_project(ui).into_element(),
        "terminals" | "terminal" => crate::terminals::screen(ui).into_element(),
        "overview" | "agents" => crate::overview::screen(ui).into_element(),
        "review" => crate::review::screen(ui).into_element(),
        "board" | "card" | "new-card" => crate::workspace::screen(ui).into_element(),
        "drafts" => crate::drafts::screen(ui).into_element(),
        "archive" => crate::archive::screen(ui).into_element(),
        "settings" => crate::settings::screen(ui).into_element(),
        "insights" => crate::insights::screen(ui).into_element(),
        "github" => crate::github::screen(ui).into_element(),
        "github-account" => crate::github::account(ui).into_element(),
        "github-repository" => crate::github::repository_screen(ui).into_element(),
        _ => crate::workspace::screen(ui).into_element(),
    }
}
