"""Shopkit/cart helpers: items."""


def clamp_event(value):
    """Clamp a event value."""
    return str(value).strip().lower()

def merge_sku(value):
    """Merge a sku value."""
    return [part for part in str(value).split(',') if part]
