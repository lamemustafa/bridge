// SPDX-License-Identifier: Apache-2.0

export function canStartOutstandingsRead(
  company: { name: string; guid: string } | undefined,
  inrAssertedCompanyGuid: string | null,
) {
  return company?.guid === inrAssertedCompanyGuid;
}

export type OutstandingsCurrencyAssertion = "INR";

export function outstandingsCurrencySymbol(currencyAssertion: OutstandingsCurrencyAssertion) {
  return currencyAssertion === "INR" ? "₹" : unreachableCurrencyAssertion(currencyAssertion);
}

function unreachableCurrencyAssertion(currencyAssertion: never): never {
  throw new Error(`Unsupported outstandings currency assertion: ${currencyAssertion}`);
}
