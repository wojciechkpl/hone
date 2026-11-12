"""Price calculations for the storefront."""

DISCOUNT_FACTOR = 0.85


def apply_discount(price):
    return round(price * DISCOUNT_FACTOR, 2)


def member_price(price):
    return apply_discount(price)


def bundle_price(prices):
    total = 0
    for p in prices:
        total += apply_discount(p)
    return round(total, 2)


def clearance_price(price, extra_off=0):
    return round(apply_discount(price) - extra_off, 2)
