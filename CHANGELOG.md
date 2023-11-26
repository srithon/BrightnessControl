# 2.1.2
- update project dependencies

# 2.1.1
- fix bug where lowering brightness below 10% would crash daemon

# 2.1.0
- fixed minor bugs
- fixed code formatting
- addressed code warnings
- updated project dependencies

# 2.0.0-alpha-2
- implement monitor-specific nightlight mode

# 2.0.0-alpha-1
- updated project dependencies
- fades are now smoother and more consistent
- the daemon now gracefully shuts down after receiving `SIGINT` (Control+c) and `SIGQUIT` signals in addition to the already-handled `SIGTERM`

# 2.0.0-alpha-0
- monitors now have separate brightness levels
- you can specify which monitor(s) a command should affect using `-m <adapter name>`, `--active`, `--enabled` or `--all`
    - the distinction between `--all` and `--enabled` is only significant for the `get` subcommand
        - `--all` will match disconnected monitors while `--enabled` will not
- add `monitors` subcommand
    - allows you to set the active monitor with `--set-active <adapter name>`
    - reconfigure displays (mentioned below)

## Breaking Changes
- changed `--get` argument into `get` subcommand
    - the options are now real arguments
- `get --brightness` now returns lines of format `<adapter name>: <brightness level>`
    - `brightness level` is a _floating-point number_ from 0 to 100
    - this is important to know when scripting, as Bash requires you to use different syntax for dealing with floating point numbers
- moved `--configure_display` to `monitors` > `--reconfigure-displays`

# 1.6.2
* implemented one central cache file instead of one per property
* fix README
  * was not yet updated for v1.6.0

# 1.6.1
* refactored codebase
* fix mode caching between runs

# 1.6.0
* this release is identical to v1.6.0-alpha-1
  * see Pull Request #34 for the reasoning

# 1.6.0-alpha-1
* Fixed multi-monitors
  * NOTE: currently cannot modify brightness individually
    * brightness commands affect ALL displays

# 1.6.0-alpha-0
* Daemon now processes input asynchronously
  * process multiple inputs at the same time
* Fades are now interruptible
  * `brightness_control b -t`
  * can use this in conjunction with increment/decrement/set to replace the current fade
    * `brightness_control b -ti10`
* Improved CLI input validation
  * arguments/flags that should not be used together now explicitly conflict
