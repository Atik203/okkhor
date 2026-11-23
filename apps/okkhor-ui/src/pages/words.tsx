import { Card, CardContent, CardHeader, CardTitle } from "../components/ui/card";
import { Badge } from "../components/ui/badge";

export default function WordsPage() {
  return (
    <div className="grid max-w-2xl gap-4">
      <Card>
        <CardHeader>
          <CardTitle>Word manager</CardTitle>
        </CardHeader>
        <CardContent className="grid gap-3">
          <p className="text-sm text-neutral-600 dark:text-neutral-300">
            <Badge variant="secondary">Coming in 0.5.1</Badge>
          </p>
          <p className="text-sm text-neutral-600 dark:text-neutral-300">
            The paginated word list, search, add/remove, and CSV/JSON
            import/export will live here. Words are stored encrypted by the
            daemon; this page will never echo other words in logs.
          </p>
        </CardContent>
      </Card>
    </div>
  );
}
