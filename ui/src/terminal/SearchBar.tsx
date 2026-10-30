import { useEffect, useRef, useState } from "react";
import type { TermSearch } from "./xterm";

/** Barra de busca sobre o terminal (Ctrl+F): Enter/Shift+Enter, maiúsculas, contador, Esc fecha. */
export function SearchBar({ search, onClose }: { search: TermSearch; onClose: () => void }) {
  const [query, setQuery] = useState("");
  const [caseSensitive, setCaseSensitive] = useState(false);
  const [results, setResults] = useState<{ index: number; count: number } | null>(null);
  const input = useRef<HTMLInputElement>(null);

  useEffect(() => {
    input.current?.focus();
    const sub = search.onResults((index, count) => setResults({ index, count }));
    return () => {
      sub.dispose();
      search.clear();
    };
  }, [search]);

  const run = (q: string, cs: boolean) => {
    if (q) search.next(q, cs, true);
    else {
      search.clear();
      setResults(null);
    }
  };

  const counter = !query || !results ? "" : results.count === 0 ? "nenhuma" : `${results.index + 1} de ${results.count}`;

  return (
    <div role="search" aria-label="Buscar no terminal" className="term-search">
      <input
        ref={input}
        type="search"
        aria-label="Texto da busca"
        placeholder="Buscar"
        value={query}
        onChange={(e) => {
          setQuery(e.target.value);
          run(e.target.value, caseSensitive);
        }}
        onKeyDown={(e) => {
          if (e.key === "Escape") {
            e.preventDefault();
            onClose();
          } else if (e.key === "Enter" && query) {
            e.preventDefault();
            if (e.shiftKey) search.previous(query, caseSensitive);
            else search.next(query, caseSensitive);
          }
        }}
      />
      <span className="term-search-count muted" aria-live="polite">
        {counter}
      </span>
      <button
        type="button"
        className="term-search-btn"
        aria-label="Diferenciar maiúsculas"
        aria-pressed={caseSensitive}
        onClick={() => {
          setCaseSensitive(!caseSensitive);
          run(query, !caseSensitive);
        }}
      >
        Aa
      </button>
      <button
        type="button"
        className="term-search-btn"
        aria-label="Anterior"
        disabled={!query}
        onClick={() => search.previous(query, caseSensitive)}
      >
        ↑
      </button>
      <button
        type="button"
        className="term-search-btn"
        aria-label="Próxima"
        disabled={!query}
        onClick={() => search.next(query, caseSensitive)}
      >
        ↓
      </button>
      <button type="button" className="term-search-btn" aria-label="Fechar busca" onClick={onClose}>
        ×
      </button>
    </div>
  );
}
