import pytest

from legacy.core.dates import parse_fuzzy_date


def test_day_precision():
    d = parse_fuzzy_date("1994-09-15")
    assert d.value == "1994-09-15"
    assert d.precision == "day"
    assert d.year == 1994


def test_month_precision():
    d = parse_fuzzy_date("1978-03")
    assert d.precision == "month"
    assert d.year == 1978


def test_year_precision():
    d = parse_fuzzy_date("1994")
    assert d.precision == "year"
    assert d.year == 1994


def test_decade_precision():
    d = parse_fuzzy_date("1970s")
    assert d.precision == "decade"
    assert d.year == 1970


def test_unknown():
    for raw in (None, "", "unknown", "  unknown  "):
        d = parse_fuzzy_date(raw)
        assert d.precision == "unknown"
        assert d.value is None
        assert d.year is None


def test_rejects_garbage():
    with pytest.raises(ValueError):
        parse_fuzzy_date("sometime in the 90s")


def test_sort_key_orders_unknown_last():
    known = parse_fuzzy_date("1994-09-15")
    unknown = parse_fuzzy_date(None)
    assert known.sort_key < unknown.sort_key
