// SPDX-License-Identifier: Apache-2.0

declare const companyIdentityKeyBrand: unique symbol;

/**
 * Opaque, endpoint-scoped identity for operator-owned UI state.
 *
 * The endpoint is part of the key because the same Tally tuple can be served
 * by a different local instance after an operator changes the connection.
 */
export type CompanyIdentityKey = string & {
  readonly [companyIdentityKeyBrand]: "CompanyIdentityKey";
};

export type CompanyIdentityKeyInput = {
  canonical_origin: string;
  company_guid: string;
  company_number: string;
  company_name: string;
  books_from_yyyymmdd: string;
};

export function companyIdentityKey(input: CompanyIdentityKeyInput): CompanyIdentityKey {
  return JSON.stringify([
    input.canonical_origin,
    input.company_guid.toLowerCase(),
    input.company_number,
    input.company_name,
    input.books_from_yyyymmdd,
  ]) as CompanyIdentityKey;
}
