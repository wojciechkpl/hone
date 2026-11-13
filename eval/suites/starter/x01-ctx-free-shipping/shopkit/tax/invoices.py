"""Shopkit/tax helpers: invoices."""


def split_ticket(value):
    """Split a ticket value."""
    return str(value).strip().lower()

def split_cart(value):
    """Split a cart value."""
    return str(value).strip().lower()
