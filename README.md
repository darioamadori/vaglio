# vaglio

*Passare al vaglio*: to look something over, carefully, before it goes through.

`vaglio` is a small terminal pane that answers one question while an AI agent writes code for
you: **what has this task changed so far?** It lists every file each of your worktrees changed
against the branch it will merge into, grouped per worktree, and opens any of them in a
delta-style diff: line numbers on both sides, syntax colours, red and green lines, and the
exact words that changed.

```
 PROJ-42 checkout rework                                           9 file
 api  feature/PROJ-42-checkout-rework                                 +412 −35
 › M src/checkout/cart.py                                               +48 −9
   A src/checkout/discounts.py                                            +120
   M src/checkout/payment.py                                           +61 −22
   A tests/checkout/test_discounts.py                                     +140
 web  feature/PROJ-42-checkout-ui                                       +44 −8
   M app/routes/checkout.tsx                                            +30 −6
   M app/components/CartSummary.tsx                                     +14 −2
 j/k muovi · ⏎ apri · r aggiorna · q esci
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

In a [herdr](https://herdr.dev) pane with no arguments, `vaglio` follows the **workspace**
instead: it reads the list of worktrees at

```
~/.local/state/herdr/plugins/dario.compri-layout/spaces/<workspace id>
```

one absolute path per line, and redraws whenever that file changes. Whatever appends to it
decides what shows. In my setup that is a Claude Code `PostToolUse` hook: every chat in the
workspace that edits a file in a worktree adds that worktree, so work across several repos
(say a backend and a frontend repo for one ticket) shows up as one list, one group per repo.
A worktree that is removed simply drops out. `VAGLIO_STATE_DIR` points it at another state
directory.

## Keys

| Key | List | Diff |
|---|---|---|
| <kbd>j</kbd> / <kbd>k</kbd>, arrows | move | scroll a line |
| <kbd>Ctrl-d</kbd> / <kbd>Ctrl-u</kbd> | half page | half page |
| <kbd>Space</kbd> / <kbd>b</kbd> | | page down / up |
| <kbd>g</kbd> / <kbd>G</kbd> | first / last file | top / bottom |
| <kbd>Enter</kbd>, <kbd>l</kbd> | open the diff | |
| <kbd>n</kbd> / <kbd>N</kbd> | | next / previous change |
| <kbd>f</kbd> | | whole file ↔ hunks only |
| <kbd>w</kbd> | | wrap long lines on / off |
| <kbd>]</kbd> / <kbd>[</kbd> | | next / previous file |
| <kbd>Esc</kbd>, <kbd>q</kbd>, <kbd>h</kbd> | | back to the list |
| <kbd>r</kbd> | refresh now | |
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
| `src/worker.rs` | background thread: file watcher and refresh timer |
| `src/app.rs` | what is on screen and how keys move it |
| `src/ui.rs` | drawing |

## Roadmap

- The pull request of each group in its header (Bitbucket first), and <kbd>p</kbd> to open it.
