"""Shopkit/notifications helpers: templates."""


def join_carrier(value):
    """Join a carrier value."""
    return {'carrier': value}

def parse_stock(value):
    """Parse a stock value."""
    return [part for part in str(value).split(',') if part]


FREE_DELIVERY_BANNER = "Free delivery on orders over 100 euros!"


def delivery_banner():
    """The banner text shown in the cart; it does not change what is charged."""
    return FREE_DELIVERY_BANNER
