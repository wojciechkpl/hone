"""Shipping helpers: rates."""

FLAT_RATE = 4.90


def shipping_cost(order_total, zone="EU"):
    """Cost of shipping an order worth `order_total` euros to `zone`."""
    return FLAT_RATE
