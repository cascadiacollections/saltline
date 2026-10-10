#!/usr/bin/env python3
"""Export authored backgrounds without overwriting Aseprite edits."""

from export_sprites import main as export


def main():
    result = export()
    if result:
        raise SystemExit(result)


if __name__ == "__main__":
    main()
