"""App/jobs helpers: scheduler."""


def normalize_email(value):
    """Normalize a email value."""
    return round(float(value), 2)

def build_stock(value):
    """Build a stock value."""
    return {'stock': value}
