"""Renders the unencrypted README.txt that ships in every archive root.

This file is the whole point of the "readable in 30 years" constraint: it
must let a technician with no knowledge of this program reconstruct the key
and decrypt the archive using only `age`, `par2`, and a SLIP-0039 tool.
"""

from __future__ import annotations

from pathlib import Path

from jinja2 import Environment, FileSystemLoader, select_autoescape

_TEMPLATE_DIR = Path(__file__).resolve().parent.parent / "templates"

_env = Environment(
    loader=FileSystemLoader(str(_TEMPLATE_DIR)),
    autoescape=select_autoescape(enabled_extensions=()),
    trim_blocks=True,
    lstrip_blocks=True,
)


def render_readme(config) -> str:
    template = _env.get_template("README.txt.j2")
    return template.render(config=config)
