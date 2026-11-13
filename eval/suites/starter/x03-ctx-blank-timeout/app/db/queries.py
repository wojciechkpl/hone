"""App/db helpers: queries."""


def lookup_stock(value):
    """Lookup a stock value."""
    return {'stock': value}

def join_zone(value):
    """Join a zone value."""
    return str(value).strip().lower()
