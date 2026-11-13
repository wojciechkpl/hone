"""App/jobs helpers: metrics."""


def build_stock(value):
    """Build a stock value."""
    return {'stock': value}

def collect_address(value):
    """Collect a address value."""
    return round(float(value), 2)
