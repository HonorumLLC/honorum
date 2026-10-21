#!/usr/bin/env python3
"""Local, opt-in API capacity test. Requires Rust (cargo), Python 3, and,
for resource limits, user systemd.

Each stage seeds accounts, starts the servers, and runs the open-loop HTTP
driver, all with `honorum-lab capacity` (crates/server/src/lab/capacity.rs),
built with `cargo build --release -p honorum --features lab --bin honorum-lab`.
Use --lab-binary and --traffic-binary to compare other honorum-lab builds.

With systemd-run, the API alone receives the CPU/memory limits in a user
scope, the open-loop driver runs outside its cgroup, and resources come from
the scope's cgroup files. Without systemd (macOS) or with --no-limits, the
servers run unconfined, limits are recorded as "none (no systemd)", and
resources come from each server's /__load/stats. No production URL,
credential, or upstream is accepted.
"""
import argparse
import datetime
import json
import os
from pathlib import Path
import platform
import shutil
import signal
import socket
import subprocess
import threading
import time
import urllib.request

ROOT = Path(__file__).resolve().parent.parent


def command(args, **kwargs):
    return subprocess.run(args, cwd=ROOT, check=True, **kwargs)


def server_stats(addr):
    with urllib.request.urlopen(f"http://{addr}/__load/stats", timeout=5) as response:
        return json.loads(response.read())


class StatsSampler(threading.Thread):
    """Polls a server's /__load/stats each second for peak resident memory."""

    def __init__(self, addr):
        super().__init__(daemon=True)
        self.addr, self.peak, self.last = addr, 0, {}
        self.done = threading.Event()

    def run(self):
        while not self.done.is_set():
            try:
                self.last = server_stats(self.addr)
                self.peak = max(self.peak, int(self.last.get("resident_bytes") or 0))
            except (OSError, ValueError):
                pass
            self.done.wait(1)

    def stop(self):
        self.done.set()
        self.join(timeout=10)
        try:
            self.last = server_stats(self.addr)
            self.peak = max(self.peak, int(self.last.get("resident_bytes") or 0))
        except (OSError, ValueError):
            pass
        return {"cpu_seconds": self.last.get("cpu_seconds"), "resident_bytes": self.last.get("resident_bytes"),
                "peak_resident_bytes": self.peak, "source": "/__load/stats"}


def tool_version(args):
    try:
        return command(args, capture_output=True, text=True).stdout.strip()
    except (OSError, subprocess.CalledProcessError):
        return "unavailable"


def cpu_model():
    cpuinfo = Path("/proc/cpuinfo")
    if cpuinfo.exists():
        return next((line.split(":", 1)[1].strip() for line in cpuinfo.read_text().splitlines() if line.startswith("model name")), "unknown")
    try:
        return subprocess.run(["sysctl", "-n", "machdep.cpu.brand_string"], capture_output=True, text=True, check=True).stdout.strip() or "unknown"
    except (OSError, subprocess.CalledProcessError):
        return "unknown"


def address():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return f"127.0.0.1:{sock.getsockname()[1]}"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--postgres", required=True, help="disposable IPv4 loopback honorum_load database URL")
    parser.add_argument("--stages", default="100,500,1000,2500,5000,10000,25000,50000,100000")
    parser.add_argument("--replicas", type=int, default=1, choices=[1,2], help="API replicas")
    parser.add_argument("--separate-worker", action="store_true", help="run jobs in a separate resource-limited process")
    parser.add_argument("--seconds", type=int, default=90)
    parser.add_argument("--warmup", type=int, default=15)
    parser.add_argument("--tasks", type=int, default=100)
    parser.add_argument("--days", type=int, default=90)
    parser.add_argument("--cpu-quota", default="25%")
    parser.add_argument("--memory", default="1G")
    parser.add_argument("--query-connections", type=int, default=16)
    parser.add_argument("--api-lock-connections", type=int, default=8)
    parser.add_argument("--worker-lock-connections", type=int, default=8)
    parser.add_argument("--worker-cpu-quota", default="")
    parser.add_argument("--worker-memory", default="")
    parser.add_argument("--no-streams", action="store_true")
    parser.add_argument("--no-limits", action="store_true", help="run servers without systemd resource limits")
    parser.add_argument("--lab-binary", type=Path, help="seeding and server honorum-lab binary (default: the Cargo build)")
    parser.add_argument("--traffic-binary", type=Path, help="traffic driver binary (default: the --lab-binary choice)")
    parser.add_argument("--out", type=Path)
    args = parser.parse_args()
    stages = [int(n) for n in args.stages.split(",")]
    if any(n < 1 or n > 100000 for n in stages) or stages != sorted(set(stages)):
        parser.error("stages must increase, with 1 to 100000 users")
    if not 1 <= args.query_connections <= 64 or not 1 <= args.api_lock_connections <= 8 or not 1 <= args.worker_lock_connections <= 8:
        parser.error("invalid connection budget")
    if args.seconds < 1 or args.warmup < 0 or not 2 <= args.tasks <= 199 or args.days < 3:
        parser.error("invalid duration, task count, or history length")
    if args.replicas > 1 and not args.separate_worker:
        parser.error("multiple API replicas require --separate-worker")
    stamp = datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    out = (args.out or ROOT / "artifacts" / "load" / stamp).resolve()
    out.mkdir(parents=True, exist_ok=False)
    data = out / "data"
    data.mkdir()
    limited = not args.no_limits and shutil.which("systemd-run") is not None
    unlimited = "none (no systemd)"
    built = Path(os.environ.get("CARGO_TARGET_DIR") or ROOT / "target")
    built = (built if built.is_absolute() else ROOT / built) / "release" / "honorum-lab"
    lab_binary = (args.lab_binary or built).resolve()
    traffic_binary = (args.traffic_binary or args.lab_binary or built).resolve()
    print(f"Results in {out}", flush=True)
    if args.lab_binary is None:
        print("Building load lab", flush=True)
        command(["cargo", "build", "--release", "--locked", "-p", "honorum", "--features", "lab", "--bin", "honorum-lab"])
    meta = {
        "commit": command(["git", "rev-parse", "HEAD"], capture_output=True, text=True).stdout.strip(),
        "host": platform.platform(),
        "cpu_quota": args.cpu_quota if limited else unlimited, "memory_limit": args.memory if limited else unlimited,
        "rustc": tool_version(["rustc", "--version"]),
        "binaries": {"lab": str(lab_binary), "traffic": str(traffic_binary)},
        "replicas": args.replicas, "separate_worker": args.separate_worker,
        "query_connections": args.query_connections, "api_lock_connections": args.api_lock_connections,
        "worker_lock_connections": args.worker_lock_connections,
        "worker_cpu_quota": (args.worker_cpu_quota or args.cpu_quota) if limited else unlimited,
        "worker_memory_limit": (args.worker_memory or args.memory) if limited else unlimited,
        "working_tree_dirty": bool(command(["git", "status", "--porcelain"], capture_output=True, text=True).stdout.strip()),
        "database": "PostgreSQL external to API cgroup" if limited else "PostgreSQL in a separate process",
        "cpu_model": cpu_model(),
        "tasks_per_account": args.tasks, "streak_days": args.days,
        "workload": "all accounts active: 1 sync/min, 1 streak/min, 1 edit/5min",
        "streams": not args.no_streams,
        "background": "all accounts have reminders; half have Canvas, 300ms simulated upstream; one forced hourly sweep",
        "exclusions": "Cloudflare edge/network, school collection, Google, calendar feeds, real SMS/push, client rendering",
        "thresholds": "each endpoint p95 <=1000ms and errors <=1%; generator lag p95 <100ms; >=99% SSE opens and no drops",
        "stages": [],
    }
    # Preserve the exact harness used, including uncommitted development edits.
    for source in ["crates/server/src/lab/capacity.rs", "crates/server/src/lab/canvas.rs", "scripts/load-test.py"]:
        (out / (Path(source).name + ".txt")).write_bytes((ROOT / source).read_bytes())
    apis = [address() for _ in range(args.replicas)]
    api, canvas = apis[0], address()
    worker = address() if args.separate_worker else ""
    config = out / "config.json"
    env = {**os.environ, "HONORUM_LOAD_CONFIG": str(config)}
    lab_args, traffic_args = [str(lab_binary), "capacity"], [str(traffic_binary), "capacity"]

    def save():
        (out / "results.json").write_text(json.dumps(meta, indent=2) + "\n")

    save()
    for users in stages:
        settings = {"Dir": str(data), "Addr": api, "CanvasAddr": canvas, "APIs": apis, "WorkerAddr": worker,
            "DatabaseURL": args.postgres, "Users": users, "Tasks": args.tasks, "Days": args.days,
            "Warmup": args.warmup, "Seconds": args.seconds, "Streams": not args.no_streams,
            "QueryConnections": args.query_connections, "APILockConnections": args.api_lock_connections,
            "WorkerLockConnections": args.worker_lock_connections}
        config.write_text(json.dumps(settings))
        config.chmod(0o600)
        print(f"Seeding {users} accounts", flush=True)
        with (out / f"seed-{users}.log").open("w") as log:
            command(lab_args, env={**env, "HONORUM_LOAD_MODE": "seed"}, stdout=log, stderr=subprocess.STDOUT)
        services = [(f"api-{i}", addr, "api" if args.separate_worker else "all") for i,addr in enumerate(apis)]
        if worker:
            services.append(("worker",worker,"worker"))
        processes = []
        groups = {}
        limits = {}
        for name,addr,role in services:
            service_config = out / f"config-{name}.json"
            service_config.write_text(json.dumps({**settings,"Addr":addr}))
            service_config.chmod(0o600)
            unit = f"honorum-load-{os.getpid()}-{users}-{name}"
            log = (out / f"server-{users}-{name}.log").open("w")
            if limited:
                server = subprocess.Popen(["systemd-run", "--user", "--scope", f"--unit={unit}",
                    "-p", f"CPUQuota={args.worker_cpu_quota or args.cpu_quota if role == 'worker' else args.cpu_quota}", "-p", f"MemoryMax={args.worker_memory or args.memory if role == 'worker' else args.memory}",
                    "-p", "MemorySwapMax=0", "env", "HONORUM_LOAD_MODE=serve", f"HONORUM_LOAD_ROLE={role}", *lab_args],
                    env={**env,"HONORUM_LOAD_CONFIG":str(service_config)}, cwd=ROOT, stdout=log, stderr=subprocess.STDOUT)
            else:
                server = subprocess.Popen(lab_args,
                    env={**env, "HONORUM_LOAD_CONFIG": str(service_config), "HONORUM_LOAD_MODE": "serve", "HONORUM_LOAD_ROLE": role},
                    cwd=ROOT, stdout=log, stderr=subprocess.STDOUT)
            processes.append((name,addr,unit,server,log))
        result = None
        samplers = {}
        try:
            for name,addr,unit,server,log in processes:
                for _ in range(300):
                    if server.poll() is not None:
                        raise RuntimeError(f"{name} exited; see server-{users}-{name}.log")
                    try:
                        with urllib.request.urlopen(f"http://{addr}/healthz", timeout=1) as response:
                            if response.status == 200:
                                break
                    except OSError:
                        time.sleep(.1)
                else:
                    raise RuntimeError(f"{name} did not become ready")
                if not limited:
                    limits[name] = unlimited
                    samplers[name] = StatsSampler(addr)
                    samplers[name].start()
                    continue
                group = command(["systemctl", "--user", "show", f"{unit}.scope", "-p", "ControlGroup", "--value"], capture_output=True, text=True).stdout.strip()
                cgroup = Path("/sys/fs/cgroup") / group.lstrip("/")
                groups[name] = cgroup
                limits[name] = {key: (cgroup / key).read_text().strip() for key in ["cpu.max", "memory.max", "memory.swap.max"]}
            print(f"Testing {users} active accounts at {users * 2.2 / 60:.1f} req/s; {limits}", flush=True)
            with (out / f"traffic-{users}.log").open("w") as traffic_log:
                command(traffic_args, env={**env, "HONORUM_LOAD_MODE": "traffic"}, stdout=traffic_log, stderr=subprocess.STDOUT,
                        timeout=args.warmup + args.seconds + max(180, users // 10))
            result = json.loads((data / "result.json").read_text())
            result["limits"] = limits
            if limited:
                result["resources"] = {name: {key: (group / key).read_text().strip() for key in ["cpu.stat", "memory.peak", "memory.events"]} for name,group in groups.items()}
            else:
                result["resources"] = {name: sampler.stop() for name, sampler in samplers.items()}
            result["database_bytes"] = result["after"].get("database_bytes")
            meta["stages"].append(result)
            save()
            s = result["summary"]
            print(f"  {'PASS' if result['pass'] else 'FAIL'} p95={s['p95_ms']:.1f}ms errors={s['error_fraction']:.2%} streams={result['streams_opened']}", flush=True)
        except Exception as error:
            meta["run_error"] = {"users": users, "message": str(error),
                                 "server_log": f"server-{users}.log", "traffic_log": f"traffic-{users}.log"}
            save()
            raise
        finally:
            for sampler in samplers.values():
                sampler.done.set()
            for name,addr,unit,server,log in processes:
                if limited:
                    subprocess.run(["systemctl", "--user", "kill", "--signal=SIGTERM", f"{unit}.scope"], capture_output=True)
                elif server.poll() is None:
                    server.send_signal(signal.SIGTERM)
            for name,addr,unit,server,log in processes:
                try:
                    server.wait(timeout=30)
                except subprocess.TimeoutExpired:
                    if limited:
                        subprocess.run(["systemctl", "--user", "kill", "--signal=SIGKILL", f"{unit}.scope"], capture_output=True)
                    else:
                        server.kill()
                    server.wait(timeout=10)
                log.close()
        if not result["pass"]:
            break
    print(f"Results: {out / 'results.json'}", flush=True)
    if not meta["stages"][-1]["pass"]:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
