<?php
final class Store {
    private PDO $db;

    public function __construct(string $path) {
        $this->db = new PDO('sqlite:' . $path, null, null, [PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION]);
        $this->db->exec('CREATE TABLE IF NOT EXISTS jobs (id INTEGER PRIMARY KEY, payload TEXT NOT NULL, status TEXT NOT NULL)');
    }

    public function create(array $payload): array {
        $statement = $this->db->prepare("INSERT INTO jobs (payload, status) VALUES (?, 'queued')");
        $statement->execute([json_encode($payload, JSON_THROW_ON_ERROR)]);
        return $this->find((int) $this->db->lastInsertId());
    }

    public function find(int $id): ?array {
        $statement = $this->db->prepare('SELECT id, payload, status FROM jobs WHERE id = ?');
        $statement->execute([$id]);
        $row = $statement->fetch(PDO::FETCH_ASSOC);
        return $row ? ['id' => (int) $row['id'], 'payload' => json_decode($row['payload'], true), 'status' => $row['status']] : null;
    }

    public function complete(int $id): ?array {
        $statement = $this->db->prepare("UPDATE jobs SET status = 'completed' WHERE id = ? AND status = 'queued'");
        $statement->execute([$id]);
        return $this->find($id);
    }

    public function counts(): array {
        return $this->db->query('SELECT status, COUNT(*) AS count FROM jobs GROUP BY status')->fetchAll(PDO::FETCH_KEY_PAIR);
    }
}
