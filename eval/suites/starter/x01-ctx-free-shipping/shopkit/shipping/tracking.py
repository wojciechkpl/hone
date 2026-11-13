"""Shopkit/shipping helpers: tracking."""


def normalize_invoice(value):
    """Normalize a invoice value."""
    return round(float(value), 2)

def parse_page(value):
    """Parse a page value."""
    return {'page': value}
