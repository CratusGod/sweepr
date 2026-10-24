# sweepr

Cleans old caches and trash from your home folder **without deleting anything right away**.

Once a month, sweepr moves files older than 30 days into a quarantine and tells you with a notification. You can undo it with one click or one command. After a 7-day grace period, the quarantine is emptied for good.

```
$ sweepr scan
RULE                ITEMS       SIZE
trash                  20     2.0 GB
thumbnails            438     4.6 MB
npm                  2771   435.8 MB
yay                    13     1.2 GB
...
TOTAL                3245     3.7 GB
```

Arch Linux. Works on any desktop with a notification daemon.

## Why another cleaner

Most cleaners treat "older than 30 days" the same way everywhere. That's wrong in two common places:

| Location | Naive cleaners | sweepr |
|---|---|---|
| **Trash** | Use the file's own date, so a 2-year-old photo you trashed *yesterday* is deleted at once | Uses the date you trashed it (`DeletionDate` in `.trashinfo`) |
| **GPU shader caches** (`~/.cache/nvidia`, Mesa) | Delete them once they stop changing, so games recompile shaders and stutter every month | Never touched. Your GPU driver manages their size |

sweepr also:
- **Never deletes during a run.** It only moves things, so every run can be undone.
- **Never copies.** Moves are instant renames. If a rename would cross filesystems (another partition or btrfs subvolume), sweepr uses a quarantine on that filesystem instead.
- **Never follows symlinks.** It never moves a file that a running program has open, and it never overwrites anything, including during restore.
- **Moves build folders whole, or not at all.** It never leaves half a git clone behind.
- **Measures age as the newer of a file's modified and changed times.** A file copied with `cp -p` or downloaded with its server date is not mistaken for an old one, and a file you restored is not swept up again on the next run.
- **Re-checks every item just before moving it.** Anything opened or touched since the scan stays where it is. One file that can't be moved never stops the rest of the run.
- **Refuses to run as root.**
- **Previews first.** Scheduled runs only preview until you run `sweepr enable`.

## Install

From the AUR (once published):

```
yay -S sweepr
```

From source:

```
git clone https://github.com/CratusGod/sweepr
cd sweepr/packaging/local && makepkg -si
```

Then, as your normal user:

```
systemctl --user enable --now sweepr.timer sweepr-tick.timer
sweepr scan -v      # see exactly what would move
sweepr enable       # let the monthly run move files
```

## Commands

| Command | What it does |
|---|---|
| `sweepr scan [-v]` | Shows what would be cleaned. Changes nothing |
| `sweepr run` | Moves old files into quarantine now |
| `sweepr restore --last` | Puts the last run back |
| `sweepr restore <run-id>` | Puts a specific run back |
| `sweepr restore --path ~/file` | Puts back one file or folder, from whichever run has it |
| `sweepr purge --expired \| --all \| <run-id>` | Deletes quarantined files for good |
| `sweepr status` | Shows quarantine contents, disk usage, and whether scheduled runs are on |
| `sweepr rules` | Lists every rule and every protected folder, with the reason it's protected |
| `sweepr enable` / `disable` | Turns on moving for scheduled runs / makes them preview-only again |

`man sweepr` (and `man sweepr-restore` etc.) has the full reference. Tab completion works in bash, zsh and fish.

Two timers run it:
- `sweepr.timer`: monthly; quarantines and notifies.
- `sweepr-tick.timer`: hourly; empties expired quarantine and warns, at most once a day, when your home disk passes 90%.

## What it cleans

| Rule | Folder | Age measured by |
|---|---|---|
| trash | `~/.local/share/Trash` | when each item was trashed |
| thumbnails, pip, go-build, yarn | `~/.cache/...` | file modified |
| electron, node-gyp | `~/.cache/...` | newest file in each download |
| npm | `~/.npm/_cacache` | file modified |
| yay, paru | AUR build folders | newest file in each package folder |
| cargo-registry | `~/.cargo/registry/cache` | file modified |
| uv, pnpm | their caches | the tool's own `prune` command (**deletes directly, cannot be restored**; `sweepr scan` lists it, turn off with `[rules.uv] enabled = false`) |
| downloads (**off** by default) | `~/Downloads` | newest file in each entry |

**Never touched:** GPU shader caches, Steam, and browser profiles and caches (Firefox, Zen, Chromium, Chrome, Brave). See `sweepr rules` for the full list.

## Configure

Copy `/usr/share/doc/sweepr/config.example.toml` to `~/.config/sweepr/config.toml`. You can:
- change the age and grace period
- turn rules on or off
- add exclude globs
- add your own folders

Unknown keys are an error, so a typo can't silently do nothing.

`exclude` patterns are matched against the path inside the rule's folder. A pattern without a `/` matches at any depth, like `.gitignore`: `*.keep` and `node_modules` work anywhere below the folder. A pattern with a `/` matches from the rule's folder, e.g. `index-v5/**` for the npm rule.

Known limit: when a restore has to recreate a folder that sweepr removed because it became empty, the new folder gets default permissions.

## Where things live

| What | Where |
|---|---|
| Quarantine | `~/.local/share/sweepr/quarantine/<run-id>/`, or `<folder>/.sweepr-quarantine/<run-id>/` when that folder is on another filesystem |
| Run records | `~/.local/state/sweepr/runs/<run-id>.json`, plus a readable `.txt` |
| Logs | `journalctl --user -u sweepr -u sweepr-tick` |

## Uninstall

```
systemctl --user disable --now sweepr.timer sweepr-tick.timer
sweepr status                # anything you want back? `sweepr restore <run-id>`
sweepr purge --all           # empties every quarantine folder, including in-folder ones
sudo pacman -Rns sweepr
rm -rf ~/.local/state/sweepr ~/.local/share/sweepr ~/.config/sweepr
```

## License

MIT
