#!/usr/bin/env python3
"""A Legatum agent in Python with no dependencies: it plays random orders from the order menu.

It shows the agent protocol (docs/agents/protocol.md), not good play. Each turn it gives each unit
one random order from the menu (never Disband), lets Scouts explore, lets each city choose its own
builds, and picks a random technology when research is idle.

    legatum-agent match --agent 0="python3 tools/agents/random_agent.py"
"""
import json
import random
import sys

NAME = "Random (Python)"
NEVER = {"DisbandUnit"}


def plan(view, menu, rng):
    orders = []
    for unit in menu["units"]:
        mine = next((u for u in view["units"] if u["id"] == unit["unit"]), None)
        if unit["kind"] == "Scout" and mine is not None and mine.get("automation") is None:
            orders.append({"type": "SetAutomation", "unit": unit["unit"], "automation": "Explore"})
            continue
        choices = [o for o in unit["orders"] if o["type"] not in NEVER]
        if choices:
            orders.append(rng.choice(choices))
    for city in menu["cities"]:
        detail = next((c.get("detail") or {} for c in view["cities"] if c["id"] == city["city"]), {})
        wanted = {"type": "SetCityAutomation", "city": city["city"], "focus": "Economy"}
        if detail.get("build_automation") is None and wanted in city["orders"]:
            orders.append(wanted)
    if view["research"].get("current") is None:
        research = [o for o in menu["empire"] if o["type"] == "SetResearch"]
        if research:
            orders.append(rng.choice(research))
    return orders


def reply(message, rng):
    """The answer to one message from the game, or None when the agent must stop."""
    kind = message.get("type")
    if kind == "hello":
        rng.seed(message["seat"]["player"])
        return {"type": "hello", "name": NAME}
    if kind == "turn":
        for refused in message.get("rejected", []):
            print(f"turn {message['turn']}: refused: {refused['message']}", file=sys.stderr)
        return {"type": "orders", "turn": message["turn"], "orders": plan(message["view"], message["menu"], rng)}
    return None


def main():
    rng = random.Random(0)
    for line in sys.stdin:
        answer = reply(json.loads(line), rng)
        if answer is None:
            return
        sys.stdout.write(json.dumps(answer, separators=(",", ":")) + "\n")
        sys.stdout.flush()


if __name__ == "__main__":
    main()
