type Tab<Id extends string> = { readonly id: Id; readonly label: string };

type Props<Id extends string> = {
  className: string;
  /** Accessible name of the tab list. */
  label: string;
  tabs: readonly Tab<Id>[];
  active: Id;
  onSelect: (id: Id) => void;
};

/** A row of tabs. Used by the top bar and by the sub-navigation inside a section. */
export function TabList<Id extends string>({ className, label, tabs, active, onSelect }: Props<Id>) {
  return (
    <nav className={className} aria-label={label}>
      {tabs.map((tab) => (
        <button
          type="button"
          key={tab.id}
          className="tab"
          aria-current={tab.id === active ? "page" : undefined}
          onClick={() => onSelect(tab.id)}
        >
          {tab.label}
        </button>
      ))}
    </nav>
  );
}
