// SPDX-License-Identifier: Apache-2.0

export function canStartOutstandingsRead(
  companyIdentityKey: string | null,
  permittedCompanyIdentity: string | null,
) {
  return companyIdentityKey !== null && companyIdentityKey === permittedCompanyIdentity;
}

export type OutstandingsCurrencyAssertion = "INR";

export function outstandingsCurrencySymbol(currencyAssertion: OutstandingsCurrencyAssertion) {
  return currencyAssertion === "INR" ? "₹" : unreachableCurrencyAssertion(currencyAssertion);
}

function unreachableCurrencyAssertion(currencyAssertion: never): never {
  throw new Error(`Unsupported outstandings currency assertion: ${currencyAssertion}`);
}
