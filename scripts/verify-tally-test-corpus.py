"""Refuse corpus qualification until independently captured evidence exists."""

import sys


def main(_argv):
    print(
        "CORPUS UNQUALIFIED: paired, partitioned voucher and opening-coverage "
        "captures are required before Bridge can accept a corpus."
    )
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
