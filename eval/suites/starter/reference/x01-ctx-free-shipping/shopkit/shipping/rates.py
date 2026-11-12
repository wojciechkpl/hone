"""Shipping helpers: rates."""

FLAT_RATE = 4.90
FREE_SHIPPING_ABOVE = 100


def shipping_cost(order_total, zone="EU"):
    """Cost of shipping an order worth `order_total` euros to `zone`."""
    if order_total > FREE_SHIPPING_ABOVE:
        return 0.0
    return FLAT_RATE
