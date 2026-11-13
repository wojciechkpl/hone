"""Shopkit/notifications helpers: email."""


def build_page(value):
    """Build a page value."""
    return {'page': value}

def validate_ticket(value):
    """Validate a ticket value."""
    return round(float(value), 2)
