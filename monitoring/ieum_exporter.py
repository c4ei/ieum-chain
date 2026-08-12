#!/usr/bin/env python3
"""Dependency-free IEUM JSON-RPC to Prometheus exporter."""
import json
import os
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.request import Request, urlopen

RPC_URL = os.getenv("IEUM_RPC_URL", "http://127.0.0.1:8989")
LISTEN = os.getenv("IEUM_EXPORTER_LISTEN", "127.0.0.1")
PORT = int(os.getenv("IEUM_EXPORTER_PORT", "9104"))


def rpc(method, params=None):
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params or []}).encode()
    with urlopen(Request(RPC_URL, body, {"Content-Type": "application/json"}), timeout=5) as response:
        payload = json.load(response)
    if "error" in payload:
        raise RuntimeError(payload["error"]["message"])
    return payload["result"]


def metric(name, value, labels=None):
    suffix = ""
    if labels:
        suffix = "{" + ",".join(f'{key}="{str(val)}"' for key, val in labels.items()) + "}"
    return f"{name}{suffix} {value}\n"


def collect():
    node = rpc("ieum_nodeStatus")
    supply = rpc("ieum_supplyStatus")
    blocks = rpc("ieum_blockProductionStatus", [100])
    validators = rpc("ieum_validatorStatus", [1000])
    output = ""
    output += metric("ieum_up", 1)
    output += metric("ieum_finalized_height", node["height"])
    output += metric("ieum_peer_count", node["peers"])
    output += metric("ieum_mempool_transactions", node["mempoolTransactions"])
    output += metric("ieum_supply_total_wei", supply["totalIssued"])
    output += metric("ieum_supply_circulating_wei", supply["circulating"])
    output += metric("ieum_supply_locked_wei", supply["locked"])
    output += metric("ieum_block_time_seconds", blocks["averageBlockTimeSeconds"])
    output += metric("ieum_missed_slots_estimated", blocks["estimatedMissedSlots"])
    for validator in validators["validators"]:
        labels = {"validator": validator["id"]}
        output += metric("ieum_validator_signing_rate_percent", validator["signingRatePercent"], labels)
        output += metric("ieum_validator_signed_blocks", validator["signedBlocks"], labels)
    return output


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path not in ("/metrics", "/healthz"):
            self.send_error(404)
            return
        try:
            body = ("ok\n" if self.path == "/healthz" else collect()).encode()
            status = 200
        except Exception as error:
            body = (metric("ieum_up", 0) + f"# exporter error: {error}\n").encode()
            status = 503
        self.send_response(status)
        self.send_header("Content-Type", "text/plain; version=0.0.4")
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, _format, *_args):
        return


HTTPServer((LISTEN, PORT), Handler).serve_forever()
