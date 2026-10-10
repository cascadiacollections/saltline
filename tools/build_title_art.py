#!/usr/bin/env python3
"""Build both title backgrounds from the original Salt & Iron hero scene."""

from build_detail_art import build as build_detail
from import_concept_art import build_title


def main():
    build_title()
    build_detail()
    print("Title background: original scene proportions, extended sky")


if __name__ == "__main__":
    main()
