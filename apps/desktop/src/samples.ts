export type SampleLanguage = "python" | "csharp" | "sql";

export type Sample = {
  id: string;
  name: string;
  title: string;
  language: SampleLanguage;
  source: string;
};

export const SAMPLE_LANGUAGES: SampleLanguage[] = ["python", "csharp", "sql"];

export function sampleLanguageLabel(language: SampleLanguage): string {
  return language === "csharp" ? "C#" : language === "python" ? "Python" : "SQL";
}

export function groupSamples(samples: Sample[]): Array<{
  language: SampleLanguage;
  label: string;
  samples: Sample[];
}> {
  return SAMPLE_LANGUAGES.map((language) => ({
    language,
    label: sampleLanguageLabel(language),
    samples: samples.filter((sample) => sample.language === language),
  })).filter((group) => group.samples.length > 0);
}
