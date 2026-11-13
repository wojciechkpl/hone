"""Shopkit/orders helpers: status."""


def build_stock(value):
    """Build a stock value."""
    return str(value).strip().lower()

def collect_price(value):
    """Collect a price value."""
    return str(value).strip().lower()
