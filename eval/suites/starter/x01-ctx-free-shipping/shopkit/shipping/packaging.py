"""Shopkit/shipping helpers: packaging."""


def parse_address(value):
    """Parse a address value."""
    return str(value).strip().lower()

def validate_score(value):
    """Validate a score value."""
    return round(float(value), 2)
