# Benchmark

This document compares the resource use of the original [caway](https://github.com/PROxZIMA/caway) script with caway-rs. It gives the method, the results, and the steps to do the benchmark again.

## Summary

With music, caway-rs used 0.6% of one CPU core. The original script used 14% to 15%. caway-rs used 4.8 MiB of memory. The original script used 7.6 to 8.6 MiB.

## Environment

| Item | Value |
|---|---|
| CPU | AMD Ryzen 5 7500F, 6 cores, 12 threads |
| OS | NixOS, Linux 7.2.6 |
| Sound server | PipeWire |
| Player | Spotify desktop client |
| cava | 1.0.0 |
| playerctl | 2.4.1 |
| Waybar | 0.15.0 |
| bash, jq | 5.3.15, 1.8.2 (recorded in November) |

Both versions used 16 bars at 30 frames per second.

## Versions under test

- **Original:** the caway script from PROxZIMA/caway. The upstream defaults are 8 bars at 60 frames per second. The tested copy used 16 bars at 30 frames per second. The only other change is the first line, which starts bash through `env`.
- **caway-rs:** version 0.1.0, release build.

The original script uses fixed paths in `/tmp`. Waybar ran a different copy of the script at the same time. To prevent interference, the tested copy used paths in a different directory.

## Method

The main benchmark measured each version alone while music played.

1. The benchmark started each version in its own cgroup with `systemd-run --user --scope`. The cgroup includes all child processes. This includes the short-lived `jq` and `sed` processes that the original script starts for each frame.
2. The benchmark waited 8 seconds. This time includes the 2 second title display and the start of cava.
3. The benchmark measured for 30 seconds:
   - **CPU:** the change of `usage_usec` in `cpu.stat`, divided by the wall time. 100% is one full CPU core.
   - **Memory:** `memory.current` at the end of the window, and `memory.peak` for the full run.
   - **Peak tasks:** the highest value of `pids.current`, read once per second. This value counts threads.
4. The order of the runs was: original, caway-rs, original, caway-rs.

Both versions wrote their output to `/dev/null`. Each cgroup also contains the `timeout` process that stops the run. This process uses almost no CPU.

## Results

### Music

Date: 2026-10-11.

| Run | CPU | Memory | Peak memory | Peak tasks |
|---|---|---|---|---|
| Original, run 1 | 15.13% | 8.6 MiB | 10.3 MiB | 12 |
| caway-rs, run 1 | 0.61% | 4.8 MiB | 5.1 MiB | 6 |
| Original, run 2 | 14.32% | 7.6 MiB | 10.1 MiB | 14 |
| caway-rs, run 2 | 0.60% | 4.8 MiB | 5.6 MiB | 6 |

The long-lived processes in run 1 had this resident memory (RSS):

| Original | RSS | caway-rs | RSS |
|---|---|---|---|
| bash, 2 processes | 4.3 and 2.4 MiB | caway | 2.5 MiB |
| playerctl | 6.9 MiB | cava | 15.5 MiB |
| cava | 15.6 MiB | | |

RSS includes shared library pages. For this reason, the sum of RSS is larger than the cgroup memory.

### CPU per process

A separate 30 second run measured the CPU time of each caway-rs process. The values come from `utime` and `stime` in `/proc/<pid>/stat`.

| Process | CPU |
|---|---|
| cava | 0.53% |
| caway | 0.07% |

Almost all of the CPU use of caway-rs comes from cava.

### Output

Both versions ran at the same time for 25 seconds on the same music. A script added a timestamp to each output line and decoded the bar heights.

| Measurement | Original | caway-rs |
|---|---|---|
| Lines per second | 30.0 | 26.4 |
| Average bar height, scale 0 to 7 | 2.25 | 2.23 |
| Bar movement per second | 5.6 | 5.3 |

Bar movement per second is the average change of one bar, in steps of the 0 to 7 scale. caway-rs does not print a frame that is the same as the previous frame. This is why it prints fewer lines. The two outputs have the same average bar height and almost the same movement.

### Silence

Date: 2026-11-06. Linux 7.2.9.

During this run, Spotify reported that it played. But no audio stream reached PipeWire, so cava received silence. These results are for this silent case only. They are not comparable with the music results.

| Run | CPU | Memory | Peak memory | Peak tasks |
|---|---|---|---|---|
| Original | 16.47% | 8.6 MiB | 10.7 MiB | 14 |
| caway-rs, run 1 | 0.16% | 4.8 MiB | 5.4 MiB | 6 |
| caway-rs, run 2 | 0.14% | 4.8 MiB | 4.9 MiB | 6 |
| caway-rs, run 3 | 0.13% | 4.8 MiB | 5.1 MiB | 6 |

caway-rs starts cava with a sleep timer. After 1 second of silence, cava stops its processing. The original script continues to process 30 frames per second.

## Limits

- All results come from one computer and one player.
- The measurement windows are short, and there are few runs.
- The CPU use of the original script depends on how fast the system starts new processes.

## Run the benchmark again

Requirements: Linux with systemd and cgroup v2, cava, playerctl, and jq.

1. Get the original script:

   ```sh
   curl -LO https://raw.githubusercontent.com/PROxZIMA/caway/master/caway
   chmod +x caway
   ```

2. If Waybar runs the original script, move the temporary paths of the test copy:

   ```sh
   sed -i "s#/tmp/#$PWD/#g" caway
   ```

3. Build caway-rs:

   ```sh
   nix build github:mostlymaxi/caway-rs
   ```

4. Save the script below as `run.sh` and make it executable.
5. Start music in the player.
6. Make sure that the audio plays on this computer. If the player sends the audio to a different device, cava receives silence.
7. Run the original script, then caway-rs:

   ```sh
   ./run.sh original bash ./caway -b 16 -f 30
   ./run.sh caway-rs ./result/bin/caway
   ```

Each run takes about 40 seconds and prints one line of results.

```bash
#!/usr/bin/env bash
# usage: run.sh <label> <cmd...>
# Runs cmd in its own cgroup, waits for warm-up, then measures a fixed window.
label=$1; shift
WARM=8; WIN=30
unit="caway-bench-$label-$$"
systemd-run --user --scope --quiet --unit="$unit" -- timeout $((WARM+WIN+3)) "$@" >/dev/null 2>&1 &
sleep 1
cg=/sys/fs/cgroup$(systemctl --user show -P ControlGroup "$unit.scope")
usage() { awk '/^usage_usec/{print $2}' "$cg/cpu.stat"; }
sleep $((WARM-1))
u0=$(usage); t0=$(date +%s%N)
maxpids=0
for i in $(seq 1 $WIN); do
  sleep 1
  p=$(cat $cg/pids.current); [ "$p" -gt "$maxpids" ] && maxpids=$p
done
u1=$(usage); t1=$(date +%s%N)
mem=$(cat $cg/memory.current); peak=$(cat $cg/memory.peak 2>/dev/null || echo 0)
procs=$(for pid in $(cat $cg/cgroup.procs); do echo "$(cat /proc/$pid/comm 2>/dev/null):$(awk '/VmRSS/{print $2}' /proc/$pid/status 2>/dev/null)kB"; done | sort | uniq -c | tr '\n' ' ')
cpu_pct=$(awk -v u=$((u1-u0)) -v t=$(( (t1-t0)/1000 )) 'BEGIN{printf "%.2f", 100*u/t}')
echo "$label cpu%=$cpu_pct cgroup_mem=$((mem/1024))kB peak=$((peak/1024))kB maxpids=$maxpids procs=[$procs]"
wait
