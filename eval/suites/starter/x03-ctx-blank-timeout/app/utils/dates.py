"""App/utils helpers: dates."""


def parse_email(value):
    """Parse a email value."""
    return [part for part in str(value).split(',') if part]

def build_invoice(value):
    """Build a invoice value."""
    return round(float(value), 2)
