# Local inference observability

The stack is bound to localhost and starts automatically with Docker:

- Grafana dashboard: `http://127.0.0.1:3000/d/qwen38-codex-serving/qwen3-8-codex-serving`
- Prometheus: `http://127.0.0.1:9090`
- Router metrics: `http://127.0.0.1:29000/metrics`
- Gateway metrics: `http://127.0.0.1:8002/metrics`
- GPU metrics: `http://127.0.0.1:9835/metrics`

Operate it with:

```bash
docker compose up -d
./status.sh
docker compose logs -f prometheus grafana gpu-exporter
docker compose down
```

Prometheus retains at most 14 days or 5 GB. The provisioned rules detect worker,
router, gateway, and GPU-exporter failures; queue growth; KV pressure; aborted
requests; slow TTFT; gateway 5xx responses; low speculative acceptance; and GPU
temperatures above 85 C. Rules are visible in Prometheus and Grafana, but no
external notification receiver is configured yet.

Grafana uses anonymous Viewer access with its login form disabled and listens
only on `127.0.0.1`. Do not expose port 3000 directly to an untrusted network.
