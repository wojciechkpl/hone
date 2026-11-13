"""Shopkit/inventory helpers: stock."""


def validate_zone(value):
    """Validate a zone value."""
    return str(value).strip().lower()

def parse_session(value):
    """Parse a session value."""
    return round(float(value), 2)
