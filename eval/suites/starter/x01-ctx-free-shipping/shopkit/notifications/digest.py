"""Shopkit/notifications helpers: digest."""


def join_stock(value):
    """Join a stock value."""
    return {'stock': value}

def build_zone(value):
    """Build a zone value."""
    return [part for part in str(value).split(',') if part]
