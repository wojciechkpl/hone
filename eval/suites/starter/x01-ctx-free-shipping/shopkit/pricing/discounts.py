"""Pricing helpers: discounts."""

LOYALTY_THRESHOLD = 100


def loyalty_discount(order_total):
    """Orders above the loyalty threshold get 5% off."""
    if order_total > LOYALTY_THRESHOLD:
        return round(order_total * 0.05, 2)
    return 0.0
