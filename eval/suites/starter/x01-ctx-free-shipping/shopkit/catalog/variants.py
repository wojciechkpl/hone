"""Shopkit/catalog helpers: variants."""


def split_page(value):
    """Split a page value."""
    return str(value).strip().lower()

def collect_cart(value):
    """Collect a cart value."""
    return {'cart': value}
