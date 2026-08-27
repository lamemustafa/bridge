import React from "react";
import { Building2, ChevronDown, Search, Settings2 } from "lucide-react";

export type ClientSwitcherClient = {
  key: string;
  name: string;
  identityDiscriminator: string;
  searchText: string;
  state: "ready" | "setup_required" | "verification_required";
};

type Props = {
  clients: ClientSwitcherClient[];
  selectedClientKey: string;
  selectionLocked: boolean;
  endpoint: string;
  endpointStatus: "checked" | "not_checked";
  loadError: string | null;
  profilesTruncated: boolean;
  loadedProfileCount: number;
  onOpen: () => void;
  onSelect: (key: string) => void;
  onManageTally: () => void;
};

const STATE_LABELS: Record<ClientSwitcherClient["state"], string> = {
  ready: "Ready",
  setup_required: "Setup required",
  verification_required: "Verification required",
};

export function ClientSwitcher({
  clients,
  selectedClientKey,
  selectionLocked,
  endpoint,
  endpointStatus,
  loadError,
  profilesTruncated,
  loadedProfileCount,
  onOpen,
  onSelect,
  onManageTally,
}: Props) {
  const [open, setOpen] = React.useState(false);
  const [query, setQuery] = React.useState("");
  const triggerRef = React.useRef<HTMLButtonElement>(null);
  const searchRef = React.useRef<HTMLInputElement>(null);
  const selected = clients.find((client) => client.key === selectedClientKey);
  const filtered = clients.filter((client) => client.searchText.toLocaleLowerCase().includes(query.trim().toLocaleLowerCase()));

  React.useEffect(() => {
    if (open) searchRef.current?.focus();
  }, [open]);

  function close() {
    setOpen(false);
    setQuery("");
    triggerRef.current?.focus();
  }

  return (
    <section className="client-switcher" aria-label="Current client">
      <div className="client-switcher-current">
        <span>Client</span>
        <strong title={selected?.name}>{selected?.name ?? "Choose a client"}</strong>
        <small>{endpointStatus === "checked" ? `Tally checked at ${endpoint}` : `Tally endpoint ${endpoint} has not been checked`}</small>
      </div>
      <div className="client-switcher-actions">
        <button
          ref={triggerRef}
          className="secondary-action client-switcher-trigger"
          type="button"
          aria-expanded={open}
          aria-controls="client-switcher-list"
          disabled={selectionLocked}
          onClick={() => {
            setOpen((current) => {
              if (!current) onOpen();
              return !current;
            });
          }}
        >
          <Building2 size={16} />
          Switch client
          <ChevronDown size={16} aria-hidden="true" />
        </button>
        <button
          className="secondary-action"
          type="button"
          onClick={() => {
            close();
            onManageTally();
          }}
        >
          <Settings2 size={16} /> Manage Tally
        </button>
      </div>
      {open && (
        <div className="client-switcher-menu" id="client-switcher-list">
          <label className="client-switcher-search">
            <Search size={16} aria-hidden="true" />
            <span className="visually-hidden">Find a client</span>
            <input
              ref={searchRef}
              type="search"
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Escape") close();
              }}
              placeholder="Find a client"
            />
          </label>
          <div className="client-switcher-list" role="list" aria-label="Available clients">
            {filtered.map((client) => (
              <button
                className={`client-switcher-option${client.key === selectedClientKey ? " selected" : ""}`}
                type="button"
                key={client.key}
                aria-pressed={client.key === selectedClientKey}
                disabled={selectionLocked}
                onClick={() => {
                  onSelect(client.key);
                  close();
                }}
              >
                <span className="client-switcher-option-details">
                  <span>{client.name}</span>
                  <small>{client.identityDiscriminator}</small>
                </span>
                <small>{STATE_LABELS[client.state]}</small>
              </button>
            ))}
            {filtered.length === 0 && !loadError && (
              <p className="client-switcher-empty">
                {profilesTruncated
                  ? `No loaded client matches that search. The fetched saved-profile page contains only the newest ${loadedProfileCount} records; an older saved client may still exist.`
                  : "No saved or discovered client matches that search."}
              </p>
            )}
          </div>
          {loadError && <p className="client-switcher-error" role="alert">{loadError}</p>}
          <p className="client-switcher-note">Ready clients use their saved, endpoint-bound identity. Setup and verification are required before other clients can be read.</p>
        </div>
      )}
    </section>
  );
}
