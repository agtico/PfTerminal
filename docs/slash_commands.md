# Slash commands

PFTerminal inherits Codex slash commands and adds product-specific vault,
provider, pane, spawn, and Task Node workflows.

## PFTerminal Commands

| Command                  | Purpose                                                       |
| ------------------------ | ------------------------------------------------------------- |
| `/model`                 | Select model/provider and effort mode                         |
| `/vault`                 | Open the encrypted credential vault action menu               |
| `/vault list`            | List credential labels and metadata without revealing secrets |
| `/vault show <label>`    | Inspect one credential's metadata                             |
| `/vault credential add`  | Add a credential through the masked secure-entry flow         |
| `/providers`             | Manage provider credentials and OpenAI Codex account login    |
| `/panes`                 | Switch user panes and create Claude Code headless panes       |
| `/spawn`                 | Open managed Nazgul/Troll/Orc orchestration                   |
| `/spawn status`          | Show the current spawn hierarchy and worker status            |
| `/spawn nazgul`          | Bind an existing user pane as the Nazgul root                 |
| `/spawn troll`           | Create a Troll under an existing parent                       |
| `/spawn orc`             | Create an Orc under an existing parent                        |
| `/tasknode`              | Open the Task Node terminal menu                              |
| `/tasknode link`         | Link a GitHub-backed Task Node account                        |
| `/tasknode status`       | Show Task Node account/session status                         |
| `/tasknode tasks [tab]`  | Show Task Node tasks; defaults to outstanding                 |
| `/tasknode outstanding`  | Show outstanding Task Node tasks                              |
| `/tasknode verification` | Show tasks waiting on verification                            |
| `/tasknode refused`      | Show refused tasks                                            |
| `/tasknode rewarded`     | Show rewarded tasks                                           |
| `/tasknode task <id>`    | Open actions for one Task Node task                           |
| `/tasknode request`      | Open the Task Node request prompt                             |
| `/tasknode request <text>` | Submit a Task Node request from inline text                 |
| `/tasknode context`      | Open Task Node context                                        |
| `/tasknode chat`         | Open Task Node chat                                           |
| `/tasknode chat <text>`  | Submit a new Task Node chat message                           |
| `/tasknode requests`     | Show active Task Node requests                                |
| `/tasknode balance`      | Show read-only Task Node balance                              |
| `/tasknode rewards`      | Show recent Task Node rewards                                 |
| `/tasknode logout`       | Log out of the Task Node terminal session                     |
| `/skills`                | Browse bundled, repo, user, and plugin skills                 |

For inherited Codex CLI slash commands, see:

<https://developers.openai.com/codex/cli/slash-commands>
