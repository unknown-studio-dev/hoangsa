#!/usr/bin/env python3
"""Thin ChromaDB sidecar — stdin/stdout JSON-RPC for thoth-mcp.

Uses chromadb.PersistentClient (embedded mode) so ChromaDB handles
embedding internally via its default all-MiniLM-L6-v2 function.
No separate ONNX load in the Rust process — saves ~2 GB RSS.

Protocol: one JSON object per line on stdin, one JSON response per line
on stdout.  stderr is free for diagnostics.

Request:  {"id": 1, "op": "query_text", "collection": "thoth_memory",
           "args": {"text": "...", "n_results": 5}}
Response: {"id": 1, "data": [...], "error": null}
"""

import json
import os
import sys
import traceback

import chromadb

_client = None
_collections = {}


def _get_client(path):
    global _client
    if _client is None:
        os.makedirs(path, exist_ok=True)
        _client = chromadb.PersistentClient(path=path)
        print(f"[chroma-sidecar] opened {path}", file=sys.stderr)
    return _client


def _get_col(path, name):
    key = (path, name)
    if key not in _collections:
        client = _get_client(path)
        _collections[key] = client.get_or_create_collection(
            name, metadata={"hnsw:space": "cosine"}
        )
    return _collections[key]


def handle(req):
    op = req["op"]
    col_name = req.get("collection", "thoth_memory")
    path = req.get("path", "")
    args = req.get("args", {})

    if op == "open":
        _get_client(path)
        return {"ok": True}

    if op == "health":
        try:
            _get_client(path).heartbeat()
            return {"healthy": True}
        except Exception:
            return {"healthy": False}

    col = _get_col(path, col_name)

    if op == "ensure_collection":
        return {"name": col.name, "count": col.count()}

    if op == "upsert":
        kwargs = {"ids": args["ids"]}
        if "documents" in args:
            kwargs["documents"] = args["documents"]
        if "metadatas" in args:
            kwargs["metadatas"] = args["metadatas"]
        col.upsert(**kwargs)
        return {"ok": True}

    if op == "query_text":
        kwargs = {
            "query_texts": [args["text"]],
            "n_results": args.get("n_results", 5),
        }
        if "where" in args and args["where"]:
            kwargs["where"] = args["where"]
        raw = col.query(**kwargs, include=["documents", "metadatas", "distances"])
        hits = []
        if raw["ids"] and raw["ids"][0]:
            for i, doc_id in enumerate(raw["ids"][0]):
                hits.append({
                    "id": doc_id,
                    "distance": raw["distances"][0][i] if raw["distances"] else 0.0,
                    "document": (raw["documents"][0][i] if raw["documents"] else None),
                    "metadata": (raw["metadatas"][0][i] if raw["metadatas"] else None),
                })
        return hits

    if op == "delete":
        ids = args.get("ids")
        where = args.get("where")
        kwargs = {}
        if ids:
            kwargs["ids"] = ids
        if where:
            kwargs["where"] = where
        if kwargs:
            col.delete(**kwargs)
        return {"ok": True}

    if op == "count":
        return {"count": col.count()}

    return {"error": f"unknown op: {op}"}


def main():
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            req = json.loads(line)
        except json.JSONDecodeError as e:
            resp = {"id": None, "data": None, "error": f"bad json: {e}"}
            print(json.dumps(resp), flush=True)
            continue

        req_id = req.get("id")
        try:
            data = handle(req)
            resp = {"id": req_id, "data": data, "error": None}
        except Exception as e:
            traceback.print_exc(file=sys.stderr)
            resp = {"id": req_id, "data": None, "error": str(e)}

        print(json.dumps(resp), flush=True)


if __name__ == "__main__":
    main()
