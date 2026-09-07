#!/usr/bin/env python3
"""Render an opt-in prompt bundle; never modify an installed config implicitly."""

import argparse
from copy import deepcopy
import hashlib
import json
from pathlib import Path


SOURCE = Path(__file__).resolve().parent


def build_bundle(catalog: dict, output: Path) -> None:
    result = deepcopy(catalog)
    models = result.get("models")
    if not isinstance(models, list) or not any(
        isinstance(model, dict) and str(model.get("slug", "")).startswith("Qwen/")
        for model in models
    ):
        raise ValueError("Expected a ZIPCODE catalog containing Qwen models")
    shared = (SOURCE / "shared.md").read_text(encoding="utf-8")
    if len(shared.encode()) > 6000:
        raise ValueError("Shared prompt exceeds the 6000-byte safety cap")
    for model in models:
        if not str(model.get("slug", "")).startswith("Qwen/"):
            continue
        messages = model.setdefault("model_messages", {})
        if messages is None:
            messages = model["model_messages"] = {}
        messages["instructions_template"] = shared
        messages["instructions_variables"] = None
        model["base_instructions"] = shared  # Compatibility with older clients.
    output = output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    (output / "shared.md").write_text(shared, encoding="utf-8")
    (output / "models.json").write_text(
        json.dumps(result, indent=2) + "\n", encoding="utf-8"
    )
    profile = f"model_instructions_file = {json.dumps(str(output / 'shared.md'), ensure_ascii=False)}\n"
    descriptions = {
        "worker": "Implement a bounded assignment and verify its result.",
        "reviewer": "Review consequential changes against acceptance criteria and evidence.",
    }
    for role, description in descriptions.items():
        instructions = (SOURCE / f"{role}.md").read_text(encoding="utf-8")
        role_path = output / f"{role}.toml"
        role_path.write_text(
            f"developer_instructions = {json.dumps(instructions, ensure_ascii=False)}\n",
            encoding="utf-8",
        )
        profile += (
            f"\n[agents.{role}]\ndescription = {json.dumps(description)}\n"
            f"config_file = {json.dumps(str(role_path), ensure_ascii=False)}\n"
        )
    (output / "profile.toml").write_text(profile, encoding="utf-8")
    (output / "manifest.json").write_text(
        json.dumps(
            {
                "version": 1,
                "shared_sha256": hashlib.sha256(shared.encode()).hexdigest(),
                "shared_bytes": len(shared.encode()),
                "token_count": None,
                "promotion": "requires tokenizer measurement and behavioral evaluation",
            },
            indent=2,
        )
        + "\n"
    )


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--catalog", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    if args.output.exists():
        parser.error("output already exists; choose a new candidate directory")
    build_bundle(json.loads(args.catalog.read_text()), args.output)
    print(f"Candidate rendered at {args.output}; no installation changed.")


if __name__ == "__main__":
    main()
