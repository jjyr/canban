# Related product workflows

Research date: 2026-11-05. This is a focused comparison of documented interaction patterns, not a hands-on benchmark or a complete market survey. Public documentation can change. Canban's differentiation below is a hypothesis to validate, not a claim that competitors lack every proposed feature.

## Comparable workflows

| Reference | Documented pattern | Implication for Canban |
| --- | --- | --- |
| Vibe Kanban | Issues organize work; agent workspaces provide execution, with multiple workspaces linkable to an issue. Agent configurations are separate from issue identity. | Kanban plus agents is an established pattern. Keep a card separate from an execution attempt and let users inspect that relationship. |
| Linear | An issue can remain assigned to a human while an agent is delegated work. Assignment/delegation changes appear in activity. Documents attach long-form context to work and offer version history and references. | Preserve a visible human owner and context provenance, rather than replacing ownership with whichever agent is currently active. |
| GitHub Copilot cloud agent | The documented coding loop starts from an issue and produces a PR for review; research can run as a separate session. | Distinguish research outputs from code artifacts. Canban's proposed local workflow should work without a remote PR while keeping explicit review identity. |
| Herdr app + core | App-owned Spaces arrange terminal/mod views; core executes durable programs and exposes their state. | Reuse execution and observation rather than building another terminal supervisor. Add the task/context workflow that Canban needs in Canban itself. |

Sources: [Vibe Kanban issue management](https://www.vibekanban.com/docs/issue-management), [agent configurations](https://www.vibekanban.com/docs/settings/agent-configurations); [Linear assignment and delegation](https://linear.app/docs/assigning-issues), [documents](https://linear.app/docs/documents); [GitHub's agent workflow](https://docs.github.com/en/copilot/how-tos/copilot-on-github/use-copilot-agents/overview); [pinned Herdr concepts](herdr-core-concepts.md).

One terminology trap is particularly relevant: Vibe Kanban's inspected Git Operations documentation uses the workspace's **Merge** action for bringing the target branch into the working branch. Canban's product idea describes accepting work into the target. Name each operation by its direction and show the affected branches; an ambiguous Merge button is insufficient. [Git Operations](https://www.vibekanban.com/docs/workspaces/git-operations).

## Product hypothesis: Human Context

The strongest proposed focus is not the presence of a board. It is the continuous connection between a person's reasoning and the executed change:

```text
What I was thinking
       |
       v
Note + selected discussion + decision
       |
       v
Task intent + acceptance criteria
       |
       v
Captured context + dispatch policy + actual agent execution
       |
       v
Change + checks + explanation + human decision
```

The card should answer both "what needs me now?" and "why did this agent make this change?" without reconstructing history across terminals, documents, and a separate PR. That is the product promise to test. It does not require inventing a new agent runtime or claiming a new model technique.

## What to borrow

- From agent Kanban tools: immediate board-to-run navigation, separate execution attempts, visible branch/worktree identity, and clear queue versus running states.
- From issue trackers: stable task identity, activity history, human ownership, useful filtering, and context references.
- From Herdr: persistent execution, agent attention patterns, terminal takeover, and a coherent global Agents view.
- From Shear: review based on a precise source comparison, semantic navigation, and comments that become explicitly outdated when source identity changes.

## What to validate before broadening scope

Use a small real feature as a later prototype study. Compare the proposed flow with the user's current notes + Herdr + Shear workflow. Record elapsed time and observed behavior; do not fabricate baseline numbers.

| Task | Evidence to collect |
| --- | --- |
| Turn an idea in a note into queued work | Time, number of manual context copies, and whether the intended preset/repository is clear |
| Explain a completed attempt | Whether the user can find exact input revisions, rule, agent, changes, and checks from its card |
| Recover after interruption | Duplicate launches, lost context, ambiguous ownership, and time to resume |
| Review and request a revision | Navigation effort and whether feedback remains attached to the correct artifact |
| Choose the next human action | Whether queued, input-needed, failed, and review-ready cards are distinguishable |

The first decision gate is whether context capture and action visibility improve the existing workflow enough to justify a new management surface. Advanced review, team collaboration, and a public extension ecosystem can wait for evidence from that loop.
