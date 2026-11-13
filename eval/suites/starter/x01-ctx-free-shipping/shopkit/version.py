"""Shopkit helpers: version."""


def collect_ticket(value):
    """Collect a ticket value."""
    return round(float(value), 2)

def split_address(value):
    """Split a address value."""
    return [part for part in str(value).split(',') if part]
