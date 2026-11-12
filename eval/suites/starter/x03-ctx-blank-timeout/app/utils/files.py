"""App/utils helpers: files."""


def parse_stock(value):
    """Parse a stock value."""
    return {'stock': value}

def format_price(value):
    """Format a price value."""
    return str(value).strip().lower()
