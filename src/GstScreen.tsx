import { FileText } from "lucide-react";

type GstReturnDraft = {
  company: string;
  financial_year: string;
  gstr1: {
    b2b_invoice_count: number;
    b2c_invoice_count: number;
    credit_debit_note_count: number;
    hsn_summary_count: number;
  };
  gstr3b: {
    outward_taxable_value: string;
    integrated_tax: string;
    central_tax: string;
    state_tax: string;
    cess: string;
  };
  missing_fields: string[];
};

type Props = {
  draft: GstReturnDraft | null;
};

// Owns: the read-only GST return readiness view (view === "gst"), which
// renders the most recently prepared draft.
//
// Deliberately does NOT own: `gstCompany`, `gstFinancialYear`,
// `dashboardError`, or the `prepareDraft` handler. The "Check GST
// Availability" trigger (company/financial-year inputs and button) lives on
// the dashboard view, not inside this one -- the dashboard reads and writes
// that state directly, so it stays in App() and is out of scope for this
// extraction. `draft` remains owned by App() and is passed down read-only.
export function GstScreen({ draft }: Props) {
  const gstDraftComplete = draft !== null && draft.missing_fields.length === 0;

  return (
    !draft || !gstDraftComplete ? (
      <article className="panel wide">
        <h2>GST calculation unavailable</h2>
        <div className="empty-state">
          <FileText size={32} />
          <strong>No verified GST draft</strong>
          <span>
            {draft
              ? draft.missing_fields.join(" ")
              : "Use GST preparation on the dashboard to check availability. Zero values are not assumed."}
          </span>
        </div>
      </article>
    ) : (
      <section className="grid">
        <article className="panel">
          <h2>GSTR-1 draft</h2>
          <dl>
            <div><dt>B2B invoices</dt><dd>{draft.gstr1.b2b_invoice_count}</dd></div>
            <div><dt>B2C invoices</dt><dd>{draft.gstr1.b2c_invoice_count}</dd></div>
            <div><dt>Credit/debit notes</dt><dd>{draft.gstr1.credit_debit_note_count}</dd></div>
            <div><dt>HSN summaries</dt><dd>{draft.gstr1.hsn_summary_count}</dd></div>
          </dl>
        </article>
        <article className="panel">
          <h2>GSTR-3B draft</h2>
          <dl>
            <div><dt>Taxable value</dt><dd>{draft.gstr3b.outward_taxable_value}</dd></div>
            <div><dt>IGST</dt><dd>{draft.gstr3b.integrated_tax}</dd></div>
            <div><dt>CGST</dt><dd>{draft.gstr3b.central_tax}</dd></div>
            <div><dt>SGST</dt><dd>{draft.gstr3b.state_tax}</dd></div>
          </dl>
        </article>

      </section>
    )
  );
}
