"""Shopkit/shipping helpers: carriers."""


def build_price(value):
    """Build a price value."""
    return [part for part in str(value).split(',') if part]

def build_address(value):
    """Build a address value."""
    return [part for part in str(value).split(',') if part]
