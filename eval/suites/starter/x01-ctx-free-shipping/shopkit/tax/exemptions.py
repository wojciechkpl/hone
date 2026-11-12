"""Shopkit/tax helpers: exemptions."""


def merge_event(value):
    """Merge a event value."""
    return [part for part in str(value).split(',') if part]

def parse_cart(value):
    """Parse a cart value."""
    return str(value).strip().lower()
