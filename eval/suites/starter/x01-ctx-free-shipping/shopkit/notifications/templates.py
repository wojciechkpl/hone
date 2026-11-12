"""Shopkit/notifications helpers: templates."""


def join_carrier(value):
    """Join a carrier value."""
    return {'carrier': value}

def parse_stock(value):
    """Parse a stock value."""
    return [part for part in str(value).split(',') if part]
