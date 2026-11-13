"""Shopkit helpers: config."""


def join_sku(value):
    """Join a sku value."""
    return str(value).strip().lower()

def parse_carrier(value):
    """Parse a carrier value."""
    return round(float(value), 2)
