import { Card, CardContent, CardHeader, CardTitle } from "../components/ui/card";
import { Badge } from "../components/ui/badge";

export default function KeyboardPage() {
  return (
    <div className="grid max-w-2xl gap-4">
      <Card>
        <CardHeader>
          <CardTitle>On-screen keyboard</CardTitle>
        </CardHeader>
        <CardContent className="grid gap-3">
          <p className="text-sm text-neutral-600 dark:text-neutral-300">
            <Badge variant="secondary">Coming in 0.5.1</Badge>
          </p>
          <p className="text-sm text-neutral-600 dark:text-neutral-300">
            The click-to-insert keyboard, layout selector, and favorites will
            live here. Nothing here writes settings or sends keystrokes today.
          </p>
        </CardContent>
      </Card>
    </div>
  );
}
