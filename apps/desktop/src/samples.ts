import { SCRIPT_LANGUAGES, type ScriptLanguage } from "./scriptLanguages";

export type Sample = {
  id: string;
  name: string;
  title: string;
  language: ScriptLanguage;
  source: string;
};

export function groupSamples(samples: Sample[]): Array<{
  language: ScriptLanguage;
  label: string;
  samples: Sample[];
}> {
  return SCRIPT_LANGUAGES.map(({ id, name }) => ({
    language: id,
    label: name,
    samples: samples.filter((sample) => sample.language === id),
  })).filter((group) => group.samples.length > 0);
}
