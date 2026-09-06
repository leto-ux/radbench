#!/usr/bin/env python3
"""
graph_failures.py — Parse radbench DUT logs by run_id and plot failures over time.

Usage:
    python3 graph_failures.py <run_id> [--log-dir logs] [--output failures.png]

The run_id is the hex identifier embedded in log filenames and in each JSON
packet's "run_id" field (e.g. 00597baf).
"""

import argparse
import json
import math
import sys
from pathlib import Path

import matplotlib.pyplot as plt
import matplotlib.ticker as ticker


# ── Log discovery ────────────────────────────────────────────────────────────

def find_log_files(log_dir: Path, run_id: str) -> list[Path]:
    """Return all per-DUT log files whose filename contains the given run_id."""
    pattern = f"*_{run_id}.log"
    matches = sorted(log_dir.glob(pattern))
    if not matches:
        # Fallback: scan every .log for the run_id inside the JSON
        for p in sorted(log_dir.glob("*.log")):
            if p.name.startswith("alarms"):
                continue
            try:
                first = p.open().readline()
                pkt = json.loads(first)
                if pkt.get("run_id") == run_id:
                    matches.append(p)
            except (json.JSONDecodeError, OSError):
                continue
    return matches


# ── Log parsing ──────────────────────────────────────────────────────────────

def parse_log(path: Path, run_id: str):
    """
    Parse a single DUT log file and return structured data for plotting.

    Returns a dict with:
        source:    str            — DUT source name (e.g. "dut-arm")
        arch:      str            — architecture (e.g. "aarch64")
        failures:  list of dict   — each failure event with ts, epoch, n, reason
        checkpoints: list of dict — every checkpoint (for timeline context)
    """
    source = None
    arch = "unknown"
    failures = []
    checkpoints = []

    with open(path) as f:
        for lineno, raw in enumerate(f, 1):
            raw = raw.strip()
            if not raw:
                continue
            try:
                pkt = json.loads(raw)
            except json.JSONDecodeError:
                continue

            # Only consider packets belonging to this run
            if pkt.get("run_id") != run_id:
                continue

            ev = pkt.get("event", {})
            ev_type = ev.get("type")

            if ev_type == "announce":
                source = pkt.get("source", source)
                arch = ev.get("arch", arch)

            elif ev_type == "checkpoint":
                cp = {
                    "ts": pkt["ts"],
                    "epoch": ev["epoch"],
                    "n": ev["n"],
                    "status": ev["status"],
                    "temp_milli": ev.get("temp_milli"),
                }
                checkpoints.append(cp)
                if ev["status"] != "ok":
                    failures.append({
                        "ts": pkt["ts"],
                        "epoch": ev["epoch"],
                        "n": ev["n"],
                        "reason": f"checkpoint {ev['status']}",
                        "kind": "mismatch",
                    })

            elif ev_type == "error":
                failures.append({
                    "ts": pkt["ts"],
                    "epoch": ev.get("epoch", 0),
                    "n": ev.get("n", 0),
                    "reason": ev.get("reason", ""),
                    "kind": "error",
                })

    return {
        "source": source or path.stem,
        "arch": arch,
        "failures": failures,
        "checkpoints": checkpoints,
    }


# ── Plotting ─────────────────────────────────────────────────────────────────

def _t0_for(d: dict) -> int:
    """Earliest timestamp in a DUT dataset (from checkpoints or failures)."""
    candidates = [cp["ts"] for cp in d["checkpoints"]]
    candidates += [f["ts"] for f in d["failures"]]
    return min(candidates) if candidates else 0


def plot_failures(run_id: str, dut_data: list[dict], output: Path):
    """
    Create a multi-panel figure:
      1. Cumulative failures over time  (linear + log₁₀ twin axis)
      2. Checkpoint pass rate per epoch  (degradation curve)
      3. Inter-failure interval / MTBF   (time between consecutive failures)
      4. Failures per epoch              (bar chart)
      5. Temperature over time           (with failure-event markers)
    """
    has_temp = any(
        cp.get("temp_milli") is not None
        for d in dut_data
        for cp in d["checkpoints"]
    )
    n_panels = 4 + int(has_temp)
    fig, axes = plt.subplots(n_panels, 1, figsize=(14, 4.2 * n_panels),
                             sharex=False)
    fig.suptitle(f"radbench — run {run_id}", fontsize=15, fontweight="bold")

    colors = plt.cm.tab10.colors

    # ── Panel 1: cumulative failures (linear + log₁₀) ────────────────────
    ax_cum = axes[0]
    ax_log = ax_cum.twinx()

    for i, d in enumerate(dut_data):
        if not d["failures"]:
            continue
        t0 = _t0_for(d)
        times_s = [(f["ts"] - t0) / 1000.0 for f in d["failures"]]
        cumulative = list(range(1, len(times_s) + 1))
        log_cum = [math.log10(c) for c in cumulative]

        label = f'{d["source"]} ({d["arch"]})'
        c = colors[i % len(colors)]

        # Linear (left axis)
        ax_cum.step(times_s, cumulative, where="post", label=label,
                    color=c, linewidth=1.5)
        ax_cum.scatter(times_s, cumulative, s=18, zorder=5,
                       color=c, edgecolors="black", linewidths=0.4)

        # Log₁₀ (right axis, dashed)
        ax_log.plot(times_s, log_cum, color=c, linewidth=1.2,
                    linestyle="--", alpha=0.7, label=f"log\u2081\u2080 {label}")

    ax_cum.set_ylabel("Cumulative failures (linear)")
    ax_log.set_ylabel("log\u2081\u2080(cumulative failures)")
    ax_cum.set_xlabel("Time since start (s)")
    ax_cum.set_title("Cumulative failures over time")
    ax_cum.yaxis.set_major_locator(ticker.MaxNLocator(integer=True))
    ax_cum.grid(True, alpha=0.3)

    # Merge legends from both axes
    h1, l1 = ax_cum.get_legend_handles_labels()
    h2, l2 = ax_log.get_legend_handles_labels()
    ax_cum.legend(h1 + h2, l1 + l2, loc="upper left", fontsize=8)

    # ── Panel 2: checkpoint pass rate per epoch (degradation curve) ───────
    ax_pass = axes[1]
    for i, d in enumerate(dut_data):
        if not d["checkpoints"]:
            continue
        # Group checkpoints by epoch
        epoch_total: dict[int, int] = {}
        epoch_ok: dict[int, int] = {}
        for cp in d["checkpoints"]:
            e = cp["epoch"]
            epoch_total[e] = epoch_total.get(e, 0) + 1
            if cp["status"] == "ok":
                epoch_ok[e] = epoch_ok.get(e, 0) + 1

        epochs = sorted(epoch_total)
        pass_rate = [epoch_ok.get(e, 0) / epoch_total[e] for e in epochs]

        label = f'{d["source"]} ({d["arch"]})'
        ax_pass.plot(epochs, pass_rate, color=colors[i % len(colors)],
                     linewidth=1, alpha=0.8, label=label)

    ax_pass.set_ylabel("Checkpoint pass rate")
    ax_pass.set_xlabel("Epoch")
    ax_pass.set_title(
        "Checkpoint pass rate per epoch (1.0 = healthy, 0.0 = fully degraded)")
    ax_pass.set_ylim(-0.05, 1.05)
    ax_pass.axhline(1.0, color="green", linewidth=0.5, alpha=0.4)
    ax_pass.axhline(0.0, color="red", linewidth=0.5, alpha=0.4)
    ax_pass.legend(loc="lower left", fontsize=9)
    ax_pass.grid(True, alpha=0.3)

    # ── Panel 3: inter-failure interval (MTBF evolution) ─────────────────
    ax_mtbf = axes[2]
    for i, d in enumerate(dut_data):
        if len(d["failures"]) < 2:
            continue
        t0 = _t0_for(d)
        fail_times = sorted((f["ts"] - t0) / 1000.0 for f in d["failures"])
        intervals = [fail_times[j] - fail_times[j - 1]
                     for j in range(1, len(fail_times))]
        midpoints = [(fail_times[j] + fail_times[j - 1]) / 2.0
                     for j in range(1, len(fail_times))]

        label = f'{d["source"]} ({d["arch"]})'
        c = colors[i % len(colors)]
        ax_mtbf.scatter(midpoints, intervals, s=20, color=c,
                        edgecolors="black", linewidths=0.4, label=label,
                        zorder=5)
        ax_mtbf.plot(midpoints, intervals, color=c, linewidth=0.8, alpha=0.5)

    ax_mtbf.set_ylabel("\u0394t between failures (s)")
    ax_mtbf.set_xlabel("Time since start (s)")
    ax_mtbf.set_title("Inter-failure interval (MTBF trend)")
    ax_mtbf.set_yscale("log")
    ax_mtbf.legend(loc="upper right", fontsize=9)
    ax_mtbf.grid(True, alpha=0.3, which="both")

    # ── Panel 4: failures per epoch (bar chart) ──────────────────────────
    ax_epoch = axes[3]
    bar_width = 0.8 / max(len(dut_data), 1)
    for i, d in enumerate(dut_data):
        if not d["failures"]:
            continue
        epoch_counts: dict[int, int] = {}
        for f in d["failures"]:
            epoch_counts[f["epoch"]] = epoch_counts.get(f["epoch"], 0) + 1
        epochs = sorted(epoch_counts)
        counts = [epoch_counts[e] for e in epochs]
        offsets = [e + (i - len(dut_data) / 2) * bar_width for e in epochs]

        label = f'{d["source"]} ({d["arch"]})'
        ax_epoch.bar(offsets, counts, width=bar_width, label=label,
                     color=colors[i % len(colors)], edgecolor="black",
                     linewidth=0.4, alpha=0.85)

    ax_epoch.set_ylabel("Failures in epoch")
    ax_epoch.set_xlabel("Epoch")
    ax_epoch.set_title("Failures per epoch")
    ax_epoch.legend(loc="upper left", fontsize=9)
    ax_epoch.grid(True, axis="y", alpha=0.3)
    ax_epoch.yaxis.set_major_locator(ticker.MaxNLocator(integer=True))

    # ── Panel 5 (optional): temperature with failure markers ─────────────
    if has_temp:
        ax_temp = axes[4]
        for i, d in enumerate(dut_data):
            temps = [(cp["ts"], cp["temp_milli"])
                     for cp in d["checkpoints"]
                     if cp.get("temp_milli") is not None]
            if not temps:
                continue
            t0 = _t0_for(d)
            t_s = [(t - t0) / 1000.0 for t, _ in temps]
            t_c = [m / 1000.0 for _, m in temps]

            label = f'{d["source"]} ({d["arch"]})'
            c = colors[i % len(colors)]
            ax_temp.plot(t_s, t_c, label=label, color=c,
                         linewidth=1, alpha=0.7)

            # Overlay failure timestamps as vertical markers
            fail_ts = [(f["ts"] - t0) / 1000.0 for f in d["failures"]]
            for ft in fail_ts:
                ax_temp.axvline(ft, color=c, linewidth=0.5, alpha=0.25)

        ax_temp.set_ylabel("Temperature (\u00b0C)")
        ax_temp.set_xlabel("Time since start (s)")
        ax_temp.set_title(
            "DUT temperature over time (vertical lines = failure events)")
        ax_temp.legend(loc="upper left", fontsize=9)
        ax_temp.grid(True, alpha=0.3)

    fig.tight_layout(rect=[0, 0, 1, 0.96])
    fig.savefig(str(output), dpi=150)
    print(f"[graph_failures] saved \u2192 {output}")
    plt.close(fig)


# ── Summary table (stdout) ───────────────────────────────────────────────────

def print_summary(run_id: str, dut_data: list[dict]):
    print(f"\n{'=' * 72}")
    print(f"  Run {run_id} \u2014 failure summary")
    print(f"{'=' * 72}")
    for d in dut_data:
        n_err = sum(1 for f in d["failures"] if f["kind"] == "error")
        n_mis = sum(1 for f in d["failures"] if f["kind"] == "mismatch")
        n_cp = len(d["checkpoints"])
        n_ok = sum(1 for cp in d["checkpoints"] if cp["status"] == "ok")
        epochs = set(cp["epoch"] for cp in d["checkpoints"])
        fail_epochs = set(f["epoch"] for f in d["failures"])

        # Compute run duration and MTBF
        t0 = _t0_for(d)
        all_ts = ([cp["ts"] for cp in d["checkpoints"]]
                  + [f["ts"] for f in d["failures"]])
        duration_s = (max(all_ts) - t0) / 1000.0 if all_ts else 0
        n_fail_total = n_err + n_mis
        mtbf_s = duration_s / n_fail_total if n_fail_total > 0 else float("inf")

        # Cross-section: fraction of epochs that had at least one failure
        cross_section = len(fail_epochs) / len(epochs) if epochs else 0

        print(f"\n  DUT: {d['source']}  arch: {d['arch']}")
        print(f"    Run duration      : {duration_s:.1f} s")
        print(f"    Checkpoints total : {n_cp}  (ok: {n_ok})")
        print(f"    Epochs seen       : {len(epochs)}")
        print(f"    Error events      : {n_err}")
        print(f"    Mismatch events   : {n_mis}")
        print(f"    Total failures    : {n_fail_total}")
        print(f"    Affected epochs   : {sorted(fail_epochs)}")
        print(f"    Epoch cross-sect. : {cross_section:.4f}  "
              f"({len(fail_epochs)}/{len(epochs)} epochs)")
        if mtbf_s != float("inf"):
            print(f"    MTBF              : {mtbf_s:.1f} s")
        else:
            print(f"    MTBF              : \u221e (no failures)")
    print()


# ── CLI ──────────────────────────────────────────────────────────────────────

def main():
    parser = argparse.ArgumentParser(
        description="Graph failures over time for a radbench DUT run.")
    parser.add_argument("run_id",
                        help="Hex run identifier (e.g. 00597baf)")
    parser.add_argument("--log-dir", default="logs",
                        help="Directory containing DUT log files (default: logs)")
    parser.add_argument("--output", default=None,
                        help="Output image path (default: failures_<run_id>.png)")
    args = parser.parse_args()

    run_id = args.run_id
    log_dir = Path(args.log_dir)
    output = Path(args.output) if args.output else Path(f"failures_{run_id}.png")

    if not log_dir.is_dir():
        print(f"error: log directory '{log_dir}' not found", file=sys.stderr)
        sys.exit(1)

    files = find_log_files(log_dir, run_id)
    if not files:
        print(f"error: no log files found for run_id '{run_id}' in {log_dir}",
              file=sys.stderr)
        sys.exit(1)

    print(f"[graph_failures] run_id={run_id}  log files: {len(files)}")
    for f in files:
        print(f"  \u2022 {f}")

    dut_data = [parse_log(f, run_id) for f in files]
    print_summary(run_id, dut_data)
    plot_failures(run_id, dut_data, output)


if __name__ == "__main__":
    main()
