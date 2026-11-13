"""Shopkit/catalog helpers: tags."""


def normalize_zone(value):
    """Normalize a zone value."""
    return round(float(value), 2)

def parse_invoice(value):
    """Parse a invoice value."""
    return str(value).strip().lower()
