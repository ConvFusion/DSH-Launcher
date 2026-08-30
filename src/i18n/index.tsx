// Lightweight i18n: React context + a typed `t()` helper. No external deps.
import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";
import { messages, type Language, type MessageKey } from "./messages";

type Params = Record<string, string | number>;

interface I18nValue {
  lang: Language;
  setLang: (lang: Language) => void;
  t: (key: MessageKey, params?: Params) => string;
}

const I18nContext = createContext<I18nValue>({
  lang: "en",
  setLang: () => {},
  t: (k) => k,
});

/** Pick the initial language: stored config wins, else the system locale. */
export function initialLanguage(stored?: string | null): Language {
  if (stored === "zh" || stored === "en") return stored;
  if (typeof navigator !== "undefined" && navigator.language?.toLowerCase().startsWith("zh")) {
    return "zh";
  }
  return "en";
}

function interpolate(template: string, params?: Params): string {
  if (!params) return template;
  return template.replace(/\{(\w+)\}/g, (m, key) =>
    key in params ? String(params[key]) : m,
  );
}

export function I18nProvider({
  lang,
  onLangChange,
  children,
}: {
  lang: Language;
  onLangChange: (lang: Language) => void;
  children: ReactNode;
}) {
  const [current, setCurrent] = useState<Language>(lang);

  // Keep in sync when the persisted config (from the backend) arrives or
  // changes — e.g. the first status push carries the saved language.
  useEffect(() => {
    setCurrent(lang);
  }, [lang]);

  const setLang = useCallback(
    (next: Language) => {
      setCurrent(next);
      onLangChange(next);
    },
    [onLangChange],
  );

  const value = useMemo<I18nValue>(() => {
    const dict = messages[current];
    return {
      lang: current,
      setLang,
      t: (key, params) => interpolate(dict[key] ?? messages.en[key] ?? key, params),
    };
  }, [current, setLang]);

  return <I18nContext.Provider value={value}>{children}</I18nContext.Provider>;
}

export function useI18n(): I18nValue {
  return useContext(I18nContext);
}
