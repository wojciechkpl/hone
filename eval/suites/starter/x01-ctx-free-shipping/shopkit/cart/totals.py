"""Cart helpers: totals."""


def cart_total(items):
    """Sum of price * quantity over the cart items."""
    return round(sum(i["price"] * i["quantity"] for i in items), 2)
