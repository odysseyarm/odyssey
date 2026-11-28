"""Reject repeated availability metadata within rendered rustdoc blocks."""

from html.parser import HTMLParser
from pathlib import Path
import sys

VOID_TAGS = frozenset(
    "area base br col embed hr img input link meta param source track wbr".split()
)


class AvailabilityParser(HTMLParser):
    def __init__(self, file: Path) -> None:
        super().__init__()
        self.file = file
        self.depth = 0
        self.block_depth: int | None = None
        self.text: list[str] = []
        self.heading = False
        self.since_headings = 0

    def handle_starttag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        if tag in VOID_TAGS:
            return
        self.depth += 1
        if "docblock" in (dict(attrs).get("class") or "").split():
            self.block_depth = self.depth
            self.text = []
            self.since_headings = 0
        if tag in {"h1", "h2", "h3", "h4", "h5", "h6"}:
            self.heading = True

    def handle_data(self, data: str) -> None:
        if self.block_depth is not None:
            self.text.append(data)
            if self.heading and data.strip() == "Since":
                self.since_headings += 1

    def handle_endtag(self, tag: str) -> None:
        if tag in VOID_TAGS:
            return
        if self.depth == self.block_depth:
            if "".join(self.text).count("Since:") + self.since_headings > 1:
                raise ValueError(f"Repeated availability metadata in {self.file}")
            self.block_depth = None
        if tag in {"h1", "h2", "h3", "h4", "h5", "h6"}:
            self.heading = False
        self.depth -= 1


def main() -> None:
    root = Path(sys.argv[1])
    for crate in ("odyssey_protocol", "odyssey_hub_client"):
        directory = root / crate
        if not directory.is_dir():
            raise FileNotFoundError(directory)
        files = list(directory.rglob("*.html"))
        if not files:
            raise FileNotFoundError(f"No rustdoc pages in {directory}")
        for file in files:
            parser = AvailabilityParser(file)
            parser.feed(file.read_text(encoding="utf-8"))
        print(f"{crate}: checked {len(files)} rendered pages")


if __name__ == "__main__":
    main()
