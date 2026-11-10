# GitHub App and device authorization

Status: device authorization and a Canban-owned app identity were requested on 2026-11-09. `jjyr/canban-app` is registered with App ID `5671006`, public Client ID `Iv23li1ImpryrkMbQjKF`, Device Flow and any-account installation availability. The user completed installation and Device Flow authorization on the private acceptance repository; real Issue/Agent/PR/merge acceptance passed. The subsequent user-selected credential store is a local file, replacing Keychain to avoid repeated access prompts; fresh sign-in, real token rotation and account restoration across AppServer restart passed with Unix `0700`/`0600` permissions. See [implementation and acceptance](../github-integration.md).

## GitHub CLI access

The [GitHub workspace](../github-workspace.md) also supports the existing gh authorization. Notifications use gh because App user tokens do not support that endpoint. Repository reads/actions use the Canban App when connected, otherwise gh. Canban does not extract or copy gh credentials. Account/access controls have moved from Settings to the GitHub page.

## Application identity

Canban uses its own **GitHub App**, `canban-app`. Do not use Herdr's application registration, default Client ID or installation URL. A GitHub App provides repository-scoped installation and fine-grained permissions for the Issue/PR product model in [GitHub Cards](github-cards.md).

The Client ID is public configuration and may be distributed with Canban. It is distinct from the numeric App ID. Allow a deployment override for a separately registered app and configure its matching installation URL as well; a Client ID alone does not identify the app's installation page.

## Registration worksheet

| Setting | Registered value |
| --- | --- |
| Owner | `jjyr` |
| Name | `canban-app` |
| Homepage | `https://github.com/jjyr/canban` |
| Enable Device Flow | Enabled |
| Expire user authorization tokens | Enabled; implement refresh token rotation |
| Callback URL | Not required by the device flow |
| Webhooks Active | Disabled for the first polling-based integration |
| Installation availability | Any account, explicitly confirmed by the user |

Registration entry: <https://github.com/settings/apps/new>.

Recommended repository permissions for the planned product capabilities:

| Permission | Planned use |
| --- | --- |
| Metadata: read | Repository identity/access and collaborator-related metadata |
| Issues: read | Search, intake, source descriptions and remote comments |
| Pull requests: read/write | Publish PRs, read discussions/reviews and explicit PR actions |
| Contents: read/write | Repository/commit information and GitHub merge delivery |
| Checks: read | CI check runs |
| Commit statuses: read | Commit status contexts |

Validate endpoint-specific requirements during implementation. Issue editing, posting Issue comments or closing Issues requires extending Issues permission to write; local comments and local removal do not require it. A GitHub App's permissions do not elevate the signed-in user's repository role, including the collaborator lookup requirements recorded in the Card design.

The first user-token integration does not need to distribute an App private key or Client Secret. Do not generate or embed either as a desktop requirement. Installation-token/server-bot authentication would be a separate future design.

## Sign-in and repository access are separate

```text
Canban GitHub page / Account & access
                   |
              Connect GitHub
                   |
              AppServer: request device code using Client ID
                   |
              UI: show user code + open GitHub verification page
                   |
              user authorizes in browser
                   |
              AppServer: poll at GitHub's requested interval
                   |
              save access/refresh credentials to the private local file
                   |
              verify user and accessible installations/repositories
                   |
             +-----+------------------------------------+
             |                                          |
      repository accessible                 installation/approval missing
             |                                          |
      connect Project source                 install/configure canban-app
             |                                on selected repositories
             +----------------------+-------------------+
                                    |
                             recheck repository access
                                    |
                          validate collaborator mode capability
                                    |
                              enable source intake
```

Device authorization signs in a user; it does not automatically install the App or grant access to every repository. Effective access is the intersection of the user's rights and the App's rights. Show installation required, organization approval pending and missing repository selection separately from sign-in errors. A target repository can be selected while creating/configuring a Project; the final connection UI remains an implementation decision.

## Token lifecycle and authority

- AppServer owns device-code polling, account verification, local credential storage, refresh and GitHub API requests. UI receives account/access state and the user-facing verification code, not access/refresh tokens. Execution machines and Agent contexts do not receive API tokens.
- User decision: persist credentials in `<CANBAN_HOME>/credentials/github.json`, normally `~/.canban/credentials/github.json`. This is a local plaintext file; on Unix use `0700` for its directory and `0600` for the file. Use atomic replacement and filesystem sync when rotating tokens. Do not use Keychain, database/configuration storage or Agent context. Git ignores credentials directories; Card archive selection excludes them. Existing Keychain credentials require one reconnect, without automatic Keychain access.
- Keep expiring user tokens enabled. GitHub currently documents an eight-hour access-token lifetime and a six-month refresh-token lifetime; honor the returned expiry metadata.
- GitHub explicitly exempts device-flow-issued tokens from requiring `client_secret` during refresh. Refresh uses Client ID, refresh token and `grant_type=refresh_token`. A hosted secret-exchange server is unnecessary for this flow.
- Serialize refresh for an account, persist the rotated access/refresh pair before publishing it to callers, and do not reuse an old refresh token. Expired/revoked authorization requires another sign-in and preserves Cards/history.
- Respect polling interval, `authorization_pending`, `slow_down`, expiry, denial and cancellation. Discard late results from a cancelled/superseded sign-in attempt.
- Disconnect stops authenticated synchronization and removes the local credentials; it does not delete local Card history. Revoking GitHub authorization/installation is a distinct explicit operation.
- Git push still uses credentials on the saved workspace machine. Device authorization for the API does not automatically configure SSH/HTTPS Git credentials there.

## Herdr reference

Read-only inspection of `~/Workspace/herdr-gpui` at `a6db99b4edab37b2038ac1016bd0af82ed0a5ebc` confirms:

- `crates/herdr-github/src/device.rs`: bounded device response handling, authorization polling replies and user profile verification.
- `crates/herdr-github/src/token.rs`: Client ID-bound access/refresh persistence and refresh without a Client Secret.
- `crates/herdr-github/src/config.rs`: Herdr-owned default Client ID, matching installation URL and configuration override.

Reuse the protocol/state-management patterns while keeping Canban identity, credential ownership and crate boundaries independent. Source inspection does not prove Canban login acceptance.

## Official sources

Verified on 2026-11-09:

- [Registering a GitHub App](https://docs.github.com/en/apps/creating-github-apps/registering-a-github-app/registering-a-github-app)
- [Generating a user access token: device flow](https://docs.github.com/en/apps/creating-github-apps/authenticating-with-a-github-app/generating-a-user-access-token-for-a-github-app#using-the-device-flow-to-generate-a-user-access-token)
- [Refreshing user access tokens](https://docs.github.com/en/apps/creating-github-apps/authenticating-with-a-github-app/refreshing-user-access-tokens#refreshing-a-user-access-token-with-a-refresh-token)
