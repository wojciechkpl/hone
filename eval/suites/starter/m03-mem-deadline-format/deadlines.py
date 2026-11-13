"""Deadline helpers for the planning tool."""

import datetime


def days_left(deadline, today=None):
    today = today or datetime.date.today()
    return (deadline - today).days
