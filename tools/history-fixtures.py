#!/usr/bin/env python3
"""Two deterministic, synthetic history workloads; no captured user data.

Usage: python3 tools/history-fixtures.py OUTPUT_DIRECTORY
Helixflow field shape: WorkflowGraph/GraphNode/GraphEdge at 2667a007.
This does not validate node catalog entries or reproduce production histories.
"""
import copy
import json
from pathlib import Path
import sys

out = Path(sys.argv[1])
out.mkdir(parents=True, exist_ok=True)

# ID move + edit: rotate a list, modify the moved item, remove and add a task.
left = [{"id": f"task-{i}", "title": f"Task {i}", "done": False,
         "priority": i % 5, "estimate": 1.5} for i in range(2000)]
right = copy.deepcopy(left[1:] + left[:1])
right[-1]["title"] = "Revised moved task"
right[-1]["done"] = True
right.pop(100)
right.insert(100, {"id": "new-task", "title": "Inserted task", "done": False,
                   "priority": 2, "estimate": 0.5})

# Actual consumer schema uses maps, so no invented ID array is introduced.
graph = {"schema_version": 1, "nodes": {
    f"node-{i}": {"node_type": "model.text", "title": f"Node {i}",
                  "params": {"temperature": 0.75, "max_tokens": 1024},
                  "pos": [float(i % 50) * 120.5, float(i // 50) * 80.0]}
    for i in range(2000)}, "edges": [
        {"from": [f"node-{i}", "text"], "to": [f"node-{i+1}", "prompt"],
         "edge_type": "data"} for i in range(1999)]}
edited = copy.deepcopy(graph)
for i in range(0, 2000, 20):
    edited["nodes"][f"node-{i}"]["pos"][0] += 20.5
    edited["nodes"][f"node-{i}"]["params"]["temperature"] = 0.5
edited["nodes"]["added"] = {"node_type": "output.text", "title": "Review",
                            "params": {}, "pos": [600.0, 400.0]}
edited["edges"].append({"from": ["node-1999", "text"], "to": ["added", "text"],
                        "edge_type": "data"})
for name, a, b in [("id-history-2000", left, right), ("helixflow-schema-2000", graph, edited)]:
    (out / f"{name}.json").write_text(
        json.dumps({"name": name, "left": a, "right": b}, separators=(",", ":")) + "\n")
