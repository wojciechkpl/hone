"""Legacy v1 delivery pricing.

Replaced by `rates.py` in v2 and only kept for the old admin importer. Nothing in the
storefront calls it any more.
"""

LEGACY_DELIVERY_CHARGE = 7.5


def legacy_delivery_charge(order_total, zone="EU"):
    """v1 charged every order for delivery; free delivery above 100 euros was a v2 idea."""
    return LEGACY_DELIVERY_CHARGE
