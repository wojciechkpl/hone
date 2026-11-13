"""App helpers: cache settings."""


def parse_ttl(text):
    """Blank means no expiry (0)."""
    text = text.strip()
    return int(text) if text else 0
