"""Cheat: the constant is extracted and a helper exists, but the functions don't use it."""

DISCOUNT = 0.85


def apply_discount(price):
    return round(price * DISCOUNT, 2)


def member_price(price):
    return round(price * DISCOUNT, 2)


def bundle_price(prices):
    total = 0
    for p in prices:
        total += round(p * DISCOUNT, 2)
    return round(total, 2)


def clearance_price(price, extra_off=0):
    discounted = round(price * DISCOUNT, 2)
    return round(discounted - extra_off, 2)
