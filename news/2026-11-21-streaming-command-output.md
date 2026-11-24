# Long commands now stream progress — and can run without a timeout

Agents no longer have to wait until `apt-get`, a build, or a kernel change finishes before they can see what is happening.

Use `ruterm_start_execute` to launch a command; it returns a job id immediately. Then call `ruterm_read_stream` with that id to receive output in chunks as the VPS produces it. The call long-polls, so progress appears as soon as there is new output instead of after the whole pipeline ends. `ruterm_cancel_stream` sends Ctrl-C when you want to stop a job.

`timeout_ms: 0` now means **no limit**, on both `start_execute` and the original `ruterm_execute`. Positive values are still capped at one hour, and the default for `execute` remains 180 seconds. For interactive tools such as `vim`, `btop` or `mc`, use `ruterm_send_keys` together with `ruterm_read_screen` rather than wrapping them in an `execute` sentinel.

Session memory still records the final exit status and a short output summary for every job, so the work log stays intact across streaming and blocking calls.
