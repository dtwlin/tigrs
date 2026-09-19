#!/usr/bin/env python3
"""Comprehensive side-by-side performance benchmark: `tigrs` (Rust) vs `tig` (C).

Benchmarked on Linux kernel repository (1,338,641 commits):
- Main Log View TTFF (Time-To-First-Frame)
- Status View TTFF
- Diff View (Show HEAD) TTFF
- Blame View (Makefile) TTFF
- First Paint Memory RSS (Resident Set Size in MB)
- Quit Latency (Responsiveness of teardown)
"""

import math
import os
import pty
import select
import subprocess
import sys
import time

import platform

TIG_BIN = os.environ.get("TIG_BIN", "tig")
TIGRS_BIN = os.environ.get("TIGRS_BIN", os.path.abspath("./target/release/tigrs"))
DEFAULT_REPO = os.environ.get("BENCH_REPO", os.path.abspath("."))


def measure_pty_run(cmd, cwd, sentinel, timeout=15.0):
  master, slave = pty.openpty()
  env = os.environ.copy()
  env["TERM"] = "xterm-256color"
  env["LANG"] = "en_US.UTF-8"
  env["XDG_CONFIG_HOME"] = "/var/empty"
  env["TIGRC_USER"] = "/dev/null"
  env["TIGRC_SYSTEM"] = "/dev/null"

  t_start = time.perf_counter()
  proc = subprocess.Popen(
      cmd,
      cwd=cwd,
      stdin=slave,
      stdout=slave,
      stderr=slave,
      close_fds=True,
      env=env,
  )
  os.close(slave)

  paint_time = None
  rss_mb = 0.0
  buf = b""
  deadline = t_start + timeout

  while time.perf_counter() < deadline:
    r, _, _ = select.select([master], [], [], 0.005)
    if r:
      try:
        chunk = os.read(master, 4096)
      except OSError:
        break
      if not chunk:
        break
      buf += chunk
      if sentinel in buf:
        paint_time = time.perf_counter() - t_start
        try:
          with open(f"/proc/{proc.pid}/status", "r") as f:
            for line in f:
              if line.startswith("VmRSS:"):
                rss_mb = int(line.split()[1]) / 1024.0
                break
        except Exception:
          pass
        break

  # Send 'q' to quit
  t_quit_sent = time.perf_counter()
  try:
    os.write(master, b"q")
  except OSError:
    pass

  try:
    proc.wait(timeout=1.0)
    quit_lat = time.perf_counter() - t_quit_sent
  except Exception:
    try:
      os.write(master, b"q")
      proc.wait(timeout=1.0)
      quit_lat = time.perf_counter() - t_quit_sent
    except Exception:
      proc.kill()
      proc.wait()
      quit_lat = -1.0

  os.close(master)
  if paint_time is None:
    paint_time = time.perf_counter() - t_start

  return paint_time * 1000.0, rss_mb, quit_lat * 1000.0


def measure_pty_interactive_navigation(cmd, cwd, sentinel, steps=20, timeout=15.0):
  master, slave = pty.openpty()
  env = os.environ.copy()
  env["TERM"] = "xterm-256color"
  env["LANG"] = "en_US.UTF-8"
  env["XDG_CONFIG_HOME"] = "/var/empty"
  env["TIGRC_USER"] = "/dev/null"
  env["TIGRC_SYSTEM"] = "/dev/null"

  t_start = time.perf_counter()
  proc = subprocess.Popen(
      cmd,
      cwd=cwd,
      stdin=slave,
      stdout=slave,
      stderr=slave,
      close_fds=True,
      env=env,
  )
  os.close(slave)

  buf = b""
  deadline = t_start + timeout

  # Wait for initial frame
  while time.perf_counter() < deadline:
    r, _, _ = select.select([master], [], [], 0.005)
    if r:
      try:
        chunk = os.read(master, 4096)
      except OSError:
        break
      if not chunk:
        break
      buf += chunk
      if sentinel in buf:
        break

  # Send `steps` navigation keystrokes and measure response latency & bytes
  step_latencies = []
  total_nav_bytes = 0
  for _ in range(steps):
    t_key = time.perf_counter()
    try:
      os.write(master, b"j")
    except OSError:
      break
    key_deadline = t_key + 2.0
    got_resp = False
    while time.perf_counter() < key_deadline:
      r, _, _ = select.select([master], [], [], 0.005)
      if r:
        try:
          chunk = os.read(master, 4096)
        except OSError:
          break
        if chunk:
          total_nav_bytes += len(chunk)
          if not got_resp:
            step_latencies.append((time.perf_counter() - t_key) * 1000.0)
            got_resp = True
        else:
          break
      elif got_resp:
        break

  # Send 'q' to quit
  t_quit_sent = time.perf_counter()
  try:
    os.write(master, b"q")
  except OSError:
    pass

  try:
    proc.wait(timeout=1.0)
    quit_lat = time.perf_counter() - t_quit_sent
  except Exception:
    try:
      os.write(master, b"q")
      proc.wait(timeout=1.0)
      quit_lat = time.perf_counter() - t_quit_sent
    except Exception:
      proc.kill()
      proc.wait()
      quit_lat = -1.0

  os.close(master)
  mean_step_lat = sum(step_latencies) / len(step_latencies) if step_latencies else 0.0
  return mean_step_lat, total_nav_bytes, quit_lat * 1000.0


def stats(values):
  n = len(values)
  mean = sum(values) / n
  stdev = math.sqrt(sum((x - mean) ** 2 for x in values) / n) if n > 1 else 0.0
  return mean, min(values), max(values), stdev


def benchmark_navigation(label, tig_args, tigrs_args, sentinel, repo, iterations=3):
  print(f"\n--- Benchmark: {label} ({iterations} iterations) ---")
  rs_lats, rs_bytes = [], []
  c_lats, c_bytes = [], []

  # Warmup
  measure_pty_interactive_navigation([TIGRS_BIN] + tigrs_args, repo, sentinel)
  measure_pty_interactive_navigation([TIG_BIN] + tig_args, repo, sentinel)

  for i in range(1, iterations + 1):
    rs_l, rs_b, _ = measure_pty_interactive_navigation([TIGRS_BIN] + tigrs_args, repo, sentinel)
    c_l, c_b, _ = measure_pty_interactive_navigation([TIG_BIN] + tig_args, repo, sentinel)

    rs_lats.append(rs_l)
    rs_bytes.append(rs_b)
    c_lats.append(c_l)
    c_bytes.append(c_b)

    print(
        f"  [{i}/{iterations}] tigrs = {rs_l:5.2f} ms/step ({rs_b:5d} B) |"
        f" tig = {c_l:5.2f} ms/step ({c_b:5d} B)"
    )

  rs_mean_l, rs_min_l, rs_max_l, rs_sd_l = stats(rs_lats)
  c_mean_l, c_min_l, c_max_l, c_sd_l = stats(c_lats)
  rs_mean_b, _, _, _ = stats(rs_bytes)
  c_mean_b, _, _, _ = stats(c_bytes)

  speedup = c_mean_l / rs_mean_l if rs_mean_l > 0 else float("inf")
  speedup_str = (
      f"{speedup:.1f}x FASTER" if speedup >= 1.0 else f"{1/speedup:.1f}x slower"
  )

  print(f"  Summary for {label}:")
  print(
      f"    tigrs: {rs_mean_l:5.2f} ± {rs_sd_l:.2f} ms/step | Total bytes: {rs_mean_b:5.0f} B"
  )
  print(
      f"    tig:   {c_mean_l:5.2f} ± {c_sd_l:.2f} ms/step | Total bytes: {c_mean_b:5.0f} B"
  )
  print(f"    RESULT: tigrs is {speedup_str} ({rs_mean_l:.2f} ms vs {c_mean_l:.2f} ms)")

  return {
      "view": label,
      "tigrs_ttff": rs_mean_l,
      "tigrs_ttff_min": rs_min_l,
      "tigrs_ttff_max": rs_max_l,
      "tig_ttff": c_mean_l,
      "tig_ttff_min": c_min_l,
      "tig_ttff_max": c_max_l,
      "tigrs_rss": rs_mean_b / 1024.0,
      "tig_rss": c_mean_b / 1024.0,
      "speedup": speedup,
  }


def benchmark_view(label, tig_args, tigrs_args, sentinel, repo, iterations=5):
  print(f"\n--- Benchmark: {label} ({iterations} iterations) ---")
  rs_ttffs, rs_rsss, rs_quits = [], [], []
  c_ttffs, c_rsss, c_quits = [], [], []

  # Warmup
  measure_pty_run([TIGRS_BIN] + tigrs_args, repo, sentinel)
  measure_pty_run([TIG_BIN] + tig_args, repo, sentinel)

  for i in range(1, iterations + 1):
    rs_t, rs_m, rs_q = measure_pty_run([TIGRS_BIN] + tigrs_args, repo, sentinel)
    c_t, c_m, c_q = measure_pty_run([TIG_BIN] + tig_args, repo, sentinel)

    rs_ttffs.append(rs_t)
    rs_rsss.append(rs_m)
    rs_quits.append(rs_q)

    c_ttffs.append(c_t)
    c_rsss.append(c_m)
    c_quits.append(c_q)

    print(
        f"  [{i}/{iterations}] tigrs = {rs_t:6.1f} ms ({rs_m:5.1f} MB) |"
        f" tig = {c_t:6.1f} ms ({c_m:5.1f} MB)"
    )

  rs_mean_t, rs_min_t, rs_max_t, rs_sd_t = stats(rs_ttffs)
  c_mean_t, c_min_t, c_max_t, c_sd_t = stats(c_ttffs)

  rs_mean_m, _, _, _ = stats(rs_rsss)
  c_mean_m, _, _, _ = stats(c_rsss)

  speedup = c_mean_t / rs_mean_t if rs_mean_t > 0 else float("inf")
  speedup_str = (
      f"{speedup:.1f}x FASTER" if speedup >= 1.0 else f"{1/speedup:.1f}x slower"
  )

  print(f"  Summary for {label}:")
  print(
      f"    tigrs TTFF: {rs_mean_t:6.1f} ± {rs_sd_t:.1f} ms [min: {rs_min_t:.1f},"
      f" max: {rs_max_t:.1f}] | RSS: {rs_mean_m:.1f} MB"
  )
  print(
      f"    tig   TTFF: {c_mean_t:6.1f} ± {c_sd_t:.1f} ms [min: {c_min_t:.1f},"
      f" max: {c_max_t:.1f}] | RSS: {c_mean_m:.1f} MB"
  )
  print(f"    RESULT:     tigrs is {speedup_str} ({rs_mean_t:.1f} ms vs {c_mean_t:.1f} ms)")

  return {
      "view": label,
      "tigrs_ttff": rs_mean_t,
      "tigrs_ttff_min": rs_min_t,
      "tigrs_ttff_max": rs_max_t,
      "tig_ttff": c_mean_t,
      "tig_ttff_min": c_min_t,
      "tig_ttff_max": c_max_t,
      "tigrs_rss": rs_mean_m,
      "tig_rss": c_mean_m,
      "speedup": speedup,
  }


def main():
  repo = sys.argv[1] if len(sys.argv) > 1 else DEFAULT_REPO

  print("=" * 76)
  print(" TIGRS vs TIG PRODUCTION PERFORMANCE BENCHMARK MATRIX")
  print(f" Repository:  {repo}")
  print(f" Host:        {platform.node()} ({platform.system()} {platform.machine()})")
  print(f" tigrs:       {TIGRS_BIN}")
  print(f" tig:         {TIG_BIN} (v2.6.0)")
  print("=" * 76)

  results = []
  results.append(
      benchmark_view("Main Log View", [], [], b"[main]", repo, iterations=5)
  )
  results.append(
      benchmark_view(
          "Status View", ["status"], ["status"], b"[status]", repo, iterations=5
      )
  )
  results.append(
      benchmark_view(
          "Diff View (HEAD)",
          ["show", "HEAD"],
          ["show", "HEAD"],
          b"[diff]",
          repo,
          iterations=5,
      )
  )
  results.append(
      benchmark_view(
          "Blame View (Makefile)",
          ["blame", "Makefile"],
          ["blame", "Makefile"],
          b"[blame]",
          repo,
          iterations=3,
      )
  )
  results.append(
      benchmark_navigation(
          "Interactive Nav (20x 'j')",
          [],
          [],
          b"[main]",
          repo,
          iterations=3,
      )
  )

  print("\n" + "=" * 76)
  print(" EXECUTIVE SUMMARY MATRIX")
  print("=" * 76)
  print(
      f"{'View':<24} | {'tigrs TTFF':<12} | {'tig TTFF':<12} | {'Speedup':<12}"
      f" | {'tigrs RSS':<10}"
  )
  print(f"{'-'*24}-|-{'-'*12}-|-{'-'*12}-|-{'-'*12}-|-{'-'*10}")
  for r in results:
    sp = (
        f"{r['speedup']:.1f}x faster"
        if r["speedup"] >= 1.0
        else f"{1/r['speedup']:.1f}x slower"
    )
    print(
        f"{r['view']:<24} | {r['tigrs_ttff']:>9.1f} ms | {r['tig_ttff']:>9.1f}"
        f" ms | {sp:>12} | {r['tigrs_rss']:>7.1f} MB"
    )
  print("=" * 76)


if __name__ == "__main__":
  main()
