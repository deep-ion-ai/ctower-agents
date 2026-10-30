#!/usr/bin/env python3
"""Confere se a versão é a mesma nos quatro arquivos de versão e nos três lockfiles.

O release-please atualiza os arquivos de versão; os lockfiles são sincronizados
pelo release.yml. Roda no CI e no fim desse passo. Sai com 1 se algo divergir.
"""
import json
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def toml(path):
    return tomllib.loads((ROOT / path).read_text(encoding="utf-8"))


def jsonf(path):
    return json.loads((ROOT / path).read_text(encoding="utf-8"))


def main():
    found = {}
    found["Cargo.toml [workspace.package]"] = toml("Cargo.toml")["workspace"]["package"]["version"]
    found["ui/package.json"] = jsonf("ui/package.json")["version"]
    found["ui/src-tauri/tauri.conf.json"] = jsonf("ui/src-tauri/tauri.conf.json")["version"]
    found["ui/src-tauri/Cargo.toml"] = toml("ui/src-tauri/Cargo.toml")["package"]["version"]

    # Nos Cargo.lock, os crates sem `source` são os nossos (path): os membros do
    # workspace e, no do app, ele mesmo e o ct-client.
    for lock in ("Cargo.lock", "ui/src-tauri/Cargo.lock"):
        ours = [p for p in toml(lock)["package"] if "source" not in p]
        if not ours:
            found[f"{lock} (crates locais)"] = None
        for p in ours:
            found[f"{lock} ({p['name']})"] = p["version"]

    npm_lock = jsonf("ui/package-lock.json")
    found["ui/package-lock.json"] = npm_lock.get("version")
    found['ui/package-lock.json (packages[""])'] = npm_lock.get("packages", {}).get("", {}).get("version")

    expected = found["Cargo.toml [workspace.package]"]
    wrong = {k: v for k, v in found.items() if v != expected}
    if wrong:
        print(f"Versões divergentes (esperado {expected}, de Cargo.toml):", file=sys.stderr)
        for k, v in wrong.items():
            print(f"  {k}: {v}", file=sys.stderr)
        print("Para sincronizar os lockfiles, veja o job lockfiles do release.yml.", file=sys.stderr)
        return 1
    print(f"Versão {expected} consistente em {len(found)} lugares.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
