# vaglio

*Passare al vaglio*: to look something over, carefully, before it goes through.

`vaglio` is a small terminal pane that answers one question while an AI agent writes code for
you: **what has this task changed so far?** It lists every file each of your worktrees changed
against the branch it will merge into, grouped per worktree, and opens any of them in a
delta-style diff: line numbers on both sides, syntax colours, red and green lines, and the
exact words that changed.

```
 PROJ-42 checkout rework                                           9 file
 ● api  feature/PROJ-42-checkout-rework                               +412 −35
   #128 DRAFT  PROJ-42: rework checkout discounts
 › M src/checkout/cart.py                                               +48 −9
   A src/checkout/discounts.py                                            +120
   M src/checkout/payment.py                                           +61 −22
   A tests/checkout/test_discounts.py                                     +140
   web  feature/PROJ-42-checkout-ui                                     +44 −8
   nessuna PR · p per crearla
   M app/routes/checkout.tsx                                            +30 −6
   M app/components/CartSummary.tsx                                     +14 −2
 j/k muovi · ⏎ apri · p PR · y copia path · r aggiorna · q esci
```

Press <kbd>Enter</kbd> and the list gives way to the diff, full pane:

```
 M src/checkout/cart.py                                             +48 −9  1/4
  3   3 │ import logging
  4     │ from datetime import datetime, timezone               ← red
      4 │ import random                                          ← green
      5 │ from datetime import datetime, timedelta, timezone    ← green, "timedelta," brighter
  5   6 │ from typing import Any
```

It is read-only on purpose. Commits, pushes and pull requests stay with whoever writes the code.

## What it compares

For each worktree, the working tree (commits, staged and unstaged changes, untracked files)
against its **merge base** with the integration branch: `origin/develop` when the repo has
one, `origin/main` otherwise. The list is therefore the pull request before it exists: the
same files, whichever chat or day produced them.

The list refreshes by itself. A file watcher re-reads a worktree the moment something in it
changes, and a three-second timer catches commits. An open diff redraws in place when the file
changes under it.

## Install

Needs a Rust toolchain (1.85+, edition 2024) and `git`.

```sh
git clone https://github.com/darioamadori/vaglio.git
cd vaglio
cargo install --path . --root ~/.local     # installs ~/.local/bin/vaglio
```

Any directory on your `PATH` works as the `--root`. After a `git pull`, run the same
`cargo install` again to update.

## Run

```sh
vaglio                         # the repo or worktree you are in
vaglio ~/wt/a ~/wt/b           # these worktrees, one group each
```

Quit with <kbd>q</kbd>.

### Inside herdr

In a [herdr](https://herdr.dev) pane with no arguments, `vaglio` follows **the chat of its
tab**, the same way the file manager next to it does. It reads two state files, one absolute
path per line, and redraws whenever either changes:

```
~/.local/state/herdr/plugins/dario.compri-layout/tabs/<tab id>          # this chat's worktree
~/.local/state/herdr/plugins/dario.compri-layout/spaces/<workspace id>  # every chat's, in the workspace
```

The tab's worktree comes first, marked `●`, and is where the selection lands; when the chat
moves to another worktree, vaglio moves with it. The rest of the workspace follows, so a task
spread over several repos (say a backend and a frontend for one ticket) is one list, one group
per repo. Until a chat of the workspace works in a worktree, the list stays empty under the
workspace's name: a main checkout is not a task's work, so vaglio does not show one. Switching
branch inside a worktree needs nothing: the header follows within seconds.

Whatever writes those files decides what shows. In my setup it is a Claude Code hook that
moves the tab only at the edges of a turn: when the prompt names a branch or ticket, and at
`Stop`, to the last worktree the chat actually worked in (an edit, a git write, a `cd` there).
A chat that only peeks at another branch leaves the pane where it is. A worktree
that is removed simply drops out. `VAGLIO_STATE_DIR` points vaglio at another state directory.

## Pull requests

Under each worktree's header, vaglio shows the pull request of its branch (number, DRAFT /
OPEN / MERGED / DECLINED, title), or **nessuna PR** in red when the branch has none yet.
<kbd>p</kbd> opens it in the browser, or the form to create it when there is none. Lookups are
refreshed every minute; <kbd>r</kbd> forces one.

Where the pull request is looked up depends on the worktree's `origin`:

- **GitHub**: through [`gh`](https://cli.github.com), with its login (`gh auth login`).
- **Bitbucket**: through the REST API, with a token of vaglio's own: an Atlassian email and API
  token, from `VAGLIO_BITBUCKET_USER` and `VAGLIO_BITBUCKET_TOKEN`, or from the macOS Keychain
  item `vaglio-bitbucket`. Create the token at
  <https://id.atlassian.com/manage-profile/security/api-tokens> (*Create API token with
  scopes*, app Bitbucket, scopes `read:pullrequest:bitbucket` and `read:repository:bitbucket`,
  nothing else). Copy it, then store it straight from the clipboard and clear the clipboard:

  ```sh
  security add-generic-password -U -s vaglio-bitbucket -a you@example.com -w "$(pbpaste)"
  pbcopy < /dev/null
  ```

  Not with a bare `-w` and the prompt it opens: that prompt keeps only the first 128
  characters, an Atlassian token is longer, and the cut-down token fails with a 401. Your shell
  history keeps the literal `$(pbpaste)`, not the token.

  The token reaches `curl` on stdin, never on its command line. To try it without opening the
  pane, run `vaglio --check-token` inside any clone whose `origin` is on Bitbucket: it prints
  `token ok`, or why Bitbucket refused it (a 401 is a wrong email or token, a 403 names the
  missing scope). If a 403 names another scope, add that one, and nothing else.

## Design documents

Pinned at the foot of the list, **design doc** counts the design documents the chats of the
workspace have written: published artifacts, Notion pages, Claude Docs and Markdown files.
<kbd>Enter</kbd> on it lists them, newest first; <kbd>Enter</kbd> on one opens it where it
belongs, not in the terminal: a Notion page in the Notion app, an artifact or a Claude Doc in the
default browser, a Markdown file in VS Code (each falling back to the system default).
<kbd>y</kbd> copies its link or path.

The list is a state file next to the other two, one JSON object per line:

```
~/.local/state/herdr/plugins/dario.compri-layout/docs/<workspace id>
{"kind": "artifact", "title": "Model bake-off", "target": "https://claude.ai/code/artifact/…", "at": 1790259848}
```

`kind` is `artifact`, `notion`, `doc` or `md`; `target` is the URL or the absolute path; the
same target written again is the same document, with its newest title. In my setup a Claude
Code `PostToolUse` hook appends a line whenever a chat publishes an artifact, creates a Notion
page or a Claude Doc, or writes a `.md` file (memory, skills, `CLAUDE.md`, `README.md` and the
like excluded). vaglio opens only what such a hook could have written: `https` links on
claude.ai or Notion, and `.md` files that still exist. The row shows only inside herdr.

## Review workspaces

A workspace where `/pr-review` was run is a review workspace from then on. Instead of the
workspace's worktrees, vaglio shows the pull request under review, with a yellow **REVIEW**
badge in every title so it is never mistaken for a task's pane:

```
 REVIEW  PR 325                                                                   6 file
   monorepo-ai  feature/CA-610-x-client-id-send-email                                +27 −3
   #325 MERGED  CA-610: forward x-client-id on ai-assistant platform actions
 › M apps/ai-assistant/ai_assistant/api/v1/endpoints/chat_stream.py                     +10
```

The hook saves what followed the command to another state file, and each new `/pr-review`
replaces it:

```
~/.local/state/herdr/plugins/dario.compri-layout/review/<workspace id>
```

vaglio reads it as a pull request link (Bitbucket `…/pull-requests/<id>` or GitHub
`…/pull/<n>`, whose branches it asks for with the same credentials as above), or else as the
first branch name it finds in the text (`feature/PROJ-42-checkout`, or a sentence naming one).
The worktree already on that branch wins, since it is what the reviewing chat reads; otherwise
it takes the repo's main checkout under `~/Developer/compri`, fetches the branch, and diffs
`origin/<branch>` against its merge base with the pull request's destination (or the
integration branch). The fetch is repeated every minute and on <kbd>r</kbd>, so new commits on
the pull request show up. A bare `/pr-review` reviews `monorepo-ai`'s current branch, as the
command does.

## Mouse

The wheel scrolls a diff three lines at a time, and moves the selection in the lists. A click
selects a file or a document, a second click on it opens it; a click on a pull request line opens
that pull request.

## Keys

| Key | List | Diff |
|---|---|---|
| <kbd>j</kbd> / <kbd>k</kbd>, arrows | move | scroll a line |
| <kbd>Ctrl-d</kbd> / <kbd>Ctrl-u</kbd> | half page | half page |
| <kbd>Space</kbd> / <kbd>b</kbd> | | page down / up |
| <kbd>g</kbd> / <kbd>G</kbd> | first / last file | top / bottom |
| <kbd>Enter</kbd>, <kbd>l</kbd> | open the diff, or the design documents list | |
| <kbd>n</kbd> / <kbd>N</kbd> | | next / previous change |
| <kbd>f</kbd> | | whole file ↔ hunks only |
| <kbd>w</kbd> | | wrap long lines on / off |
| <kbd>]</kbd> / <kbd>[</kbd> | | next / previous file |
| <kbd>Esc</kbd>, <kbd>q</kbd>, <kbd>h</kbd> | | back to the list |
| <kbd>p</kbd> | open the pull request | open the pull request |
| <kbd>y</kbd> | copy the file's path, relative to its worktree | same |
| <kbd>r</kbd> | refresh now, pull requests included | |
| <kbd>q</kbd> | quit | |

A diff opens on the whole file, scrolled to its first change; <kbd>f</kbd> narrows it to the
hunks with three lines of context.

## How it draws

- **git** does the diffing (`git diff -M <merge-base>`, `--no-index` for new files), so renames,
  your ignore rules and your git config behave as usual. External diff drivers and textconv
  are switched off, so the output is always a plain unified diff.
- **[syntect](https://github.com/trishume/syntect)** with the extra syntaxes from
  [two-face](https://github.com/CosmicHorrorDev/two-face) colours the code, with the Monokai
  Extended theme, the same stack [delta](https://github.com/dandavison/delta) uses. Each hunk
  is highlighted as two streams, the old file and the new one, so a deleted line never
  confuses the parser state of the lines around it.
- **[similar](https://github.com/mitsuhiko/similar)** finds the changed words. A deleted line is
  paired with the added line that resembles it most, not simply the next one, and a pair that
  shares too little stays plain red and green instead of turning into a wall of emphasis.
- **[ratatui](https://ratatui.rs)** draws it, with delta's dark-mode background colours.

## Development

```sh
cargo run -- ~/some/worktree
cargo build --release
```

`vaglio --snapshot 100x30 KEYS [PATH...]` renders one frame after replaying `KEYS` (a newline
is <kbd>Enter</kbd>, `~` is <kbd>Esc</kbd>) and prints it with ANSI colours. It is handy for
checking the drawing without a terminal, or from a script:

```sh
vaglio --snapshot 90x24 $'jj\n' ~/some/worktree            # open the third file
vaglio --snapshot 90x24 $'\nfn' ~/some/worktree | less -R   # hunks only, second change
```

| File | Does |
|---|---|
| `src/source.rs` | which worktrees to show: arguments, the herdr workspace list, or the current repo |
| `src/git.rs` | changed files and per-file diffs |
| `src/diff.rs` | parses a unified diff into rows: line numbers, syntax colours, changed words |
| `src/pr.rs` | the pull request of a branch, on Bitbucket or GitHub |
| `src/docs.rs` | the workspace's design documents, and opening each in its own app |
| `src/worker.rs` | background thread: file watcher, refresh timer, pull request cache |
| `src/app.rs` | what is on screen and how keys move it |
| `src/ui.rs` | drawing |

