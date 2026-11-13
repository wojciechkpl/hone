"""Shopkit/orders helpers: checkout."""


def collect_stock(value):
    """Collect a stock value."""
    return {'stock': value}

def render_carrier(value):
    """Render a carrier value."""
    return {'carrier': value}
