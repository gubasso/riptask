# Problem Statement

Status: active (permanent)

A software engineer working across multiple personal + work projects, that can be (self-hosted or not):

- GitLab projects
- GitHub projects
- Non-GitHub/GitLab projects (e.g. Codeberg, Gitea, gitolite, etc)

Has no centralized way to manage all their issues. Each platform has its own UI, its own kanban, its own project context. Switching between them to get an overview of all work in flight is friction-heavy and browser-dependent.

**The need:**

- A single pane of glass for all issues across all projects and platforms
- A kanban board view, browsable with standard Unix tools
- Full offline capability — no browser, no network required to read/manage issues
- Bidirectional sync: changes made locally reflect on remote; changes on remote pull down locally
- Support for local-only tasks not associated with any remote project
- Works naturally in a terminal-centric, editor-first workflow (Neovim, kitty, bash)

**What this is not:**
- A replacement for GitLab/GitHub as the upstream issue tracker
- A team collaboration tool
- A SaaS product
