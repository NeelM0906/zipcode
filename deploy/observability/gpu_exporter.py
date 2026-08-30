#!/usr/bin/env python3
"""Minimal localhost Prometheus exporter for NVIDIA GPU operating metrics."""

from __future__ import annotations

import csv
import subprocess
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from io import StringIO


HOST = "127.0.0.1"
PORT = 9835
QUERY = (
    "uuid,name,temperature.gpu,power.draw,power.limit,utilization.gpu,"
    "memory.used,memory.total,clocks.sm,clocks.mem,pcie.link.gen.current,"
    "pcie.link.width.current"
)
METRICS = (
    ("qwen_gpu_temperature_celsius", "GPU temperature in Celsius.", 2, 1.0),
    ("qwen_gpu_power_draw_watts", "Current GPU power draw in watts.", 3, 1.0),
    ("qwen_gpu_power_limit_watts", "Configured GPU power limit in watts.", 4, 1.0),
    ("qwen_gpu_utilization_ratio", "GPU compute utilization ratio.", 5, 0.01),
    ("qwen_gpu_memory_used_bytes", "Allocated GPU memory in bytes.", 6, 1024.0 * 1024.0),
    ("qwen_gpu_memory_total_bytes", "Total GPU memory in bytes.", 7, 1024.0 * 1024.0),
    ("qwen_gpu_sm_clock_hz", "Current SM clock in hertz.", 8, 1_000_000.0),
    ("qwen_gpu_memory_clock_hz", "Current memory clock in hertz.", 9, 1_000_000.0),
    ("qwen_gpu_pcie_generation", "Current PCIe generation.", 10, 1.0),
    ("qwen_gpu_pcie_link_width", "Current PCIe link width.", 11, 1.0),
)


def escape_label(value: str) -> str:
    return value.replace("\\", "\\\\").replace("\n", "\\n").replace('"', '\\"')


def collect() -> str:
    completed = subprocess.run(
        ["nvidia-smi", f"--query-gpu={QUERY}", "--format=csv,noheader,nounits"],
        check=True,
        capture_output=True,
        text=True,
        timeout=5,
    )
    rows = list(csv.reader(StringIO(completed.stdout), skipinitialspace=True))
    lines = [
        "# HELP qwen_gpu_exporter_up Whether the latest NVIDIA query succeeded.",
        "# TYPE qwen_gpu_exporter_up gauge",
        "qwen_gpu_exporter_up 1",
    ]
    for name, help_text, _, _ in METRICS:
        lines.extend((f"# HELP {name} {help_text}", f"# TYPE {name} gauge"))
    for index, row in enumerate(rows):
        labels = f'gpu="{index}",uuid="{escape_label(row[0])}",name="{escape_label(row[1])}"'
        for metric, _, column, scale in METRICS:
            try:
                value = float(row[column]) * scale
            except (IndexError, ValueError):
                continue
            lines.append(f"{metric}{{{labels}}} {value:.6f}")
    return "\n".join(lines) + "\n"


class Handler(BaseHTTPRequestHandler):
    def do_GET(self) -> None:
        if self.path == "/health":
            self.send_response(200)
            self.end_headers()
            self.wfile.write(b"ok\n")
            return
        if self.path != "/metrics":
            self.send_response(404)
            self.end_headers()
            return
        try:
            body = collect().encode()
            status = 200
        except (OSError, subprocess.SubprocessError):
            body = b"qwen_gpu_exporter_up 0\n"
            status = 503
        self.send_response(status)
        self.send_header("Content-Type", "text/plain; version=0.0.4; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, format: str, *args: object) -> None:
        return


if __name__ == "__main__":
    ThreadingHTTPServer((HOST, PORT), Handler).serve_forever()
